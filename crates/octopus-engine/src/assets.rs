//! 资产库（#28）：图片走文件系统内容寻址存储。
//!
//! 设计取舍（与 #27 单库 / #24 自包含存档配套）：
//! - **文件名 = `<sha256>.<ext>`，内容即身份** → 天然去重、不可变、同一张图被多处引用只存一份；
//! - **不建数据库表**：hash 就是主键、磁盘即索引，因此本特性不需要任何迁移；
//! - **发布 / 存档冻结无需拷贝资产**：引用即持有，资产永不改写；
//! - **导出才把图打进 zip**：磁盘上不重复，交付物仍自包含（#24 决议的延伸）。
//!
//! 磁盘布局：`<root>/<hash 前 2 位>/<hash>.<ext>`（分片，避免单目录堆几十万文件）。

use std::collections::BTreeSet;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use octopus_types::{SavePackage, StorybookPackage};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::EngineError;

/// 单张图片上限：前端上传前已按用途压缩，这里是兜底
pub const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;
/// 存档包内的清单条目名
pub const BUNDLE_MANIFEST: &str = "save.json";
/// 故事书包内的清单条目名
pub const BOOK_BUNDLE_MANIFEST: &str = "storybook.json";
/// 存档包内的资产目录前缀
pub const BUNDLE_ASSET_DIR: &str = "assets/";

/// 按魔数判定图片类型（不信任 Content-Type / 扩展名）
fn sniff_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && (&bytes[8..12] == b"avif" || &bytes[8..12] == b"avis") {
        return Some("avif");
    }
    None
}

/// 资产名 → HTTP Content-Type
pub fn content_type_of(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("png") => "image/png",
        Some("jpg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("avif") => "image/avif",
        _ => "application/octet-stream",
    }
}

/// 资产名合法性：`<64 位小写 hex>.<已知扩展名>`。
/// 严格校验是为了让名字可以直接拼进路径而不发生目录穿越。
pub fn is_valid_asset_name(name: &str) -> bool {
    let Some((hash, ext)) = name.split_once('.') else {
        return false;
    };
    hash.len() == 64
        && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && matches!(ext, "png" | "jpg" | "webp" | "gif" | "avif")
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 磁盘布局：分片目录 / 文件名。仅在名字已校验后调用。
fn path_in(root: &Path, name: &str) -> PathBuf {
    root.join(&name[0..2]).join(name)
}

#[derive(Debug, Clone)]
pub struct StoredAsset {
    pub name: String,
    pub size: usize,
}

pub struct AssetStore {
    root: PathBuf,
}

impl AssetStore {
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, EngineError> {
        let root = root.into();
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| EngineError::Storage(format!("创建资产目录失败 {}: {e}", root.display())))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 落盘。已存在同 hash 文件即直接命中（去重），不重复写。
    pub async fn put(&self, bytes: &[u8]) -> Result<StoredAsset, EngineError> {
        if bytes.is_empty() {
            return Err(EngineError::InvalidAsset("空文件".to_string()));
        }
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(EngineError::InvalidAsset(format!(
                "图片超过 {}MB 上限",
                MAX_ASSET_BYTES / 1024 / 1024
            )));
        }
        let ext = sniff_ext(bytes).ok_or_else(|| {
            EngineError::InvalidAsset("不是受支持的图片（png / jpg / webp / gif / avif）".to_string())
        })?;
        let name = format!("{}.{}", hex_sha256(bytes), ext);
        let path = path_in(&self.root, &name);
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            if let Some(dir) = path.parent() {
                tokio::fs::create_dir_all(dir)
                    .await
                    .map_err(|e| EngineError::Storage(format!("创建分片目录失败: {e}")))?;
            }
            // 先写临时文件再改名：避免中断/并发留下半截资产
            let tmp = path.with_extension(format!("{ext}.part"));
            tokio::fs::write(&tmp, bytes)
                .await
                .map_err(|e| EngineError::Storage(format!("写入资产失败: {e}")))?;
            tokio::fs::rename(&tmp, &path)
                .await
                .map_err(|e| EngineError::Storage(format!("落定资产失败: {e}")))?;
        }
        Ok(StoredAsset { name, size: bytes.len() })
    }

    pub async fn read(&self, name: &str) -> Result<Vec<u8>, EngineError> {
        if !is_valid_asset_name(name) {
            return Err(EngineError::InvalidAsset(format!("非法资产名: {name}")));
        }
        tokio::fs::read(path_in(&self.root, name))
            .await
            .map_err(|_| EngineError::InvalidAsset(format!("资产不存在: {name}")))
    }

    /// 同步读取（打包时用；走 spawn_blocking 之外的简单路径）
    pub fn read_blocking(&self, name: &str) -> Option<Vec<u8>> {
        if !is_valid_asset_name(name) {
            return None;
        }
        std::fs::read(path_in(&self.root, name)).ok()
    }

    /// 把解包出来的资产写回库（按内容重新校验 + 去重）
    pub async fn ingest(&self, items: &[(String, Vec<u8>)]) -> Result<usize, EngineError> {
        let mut n = 0;
        for (declared, bytes) in items {
            let stored = self.put(bytes).await?;
            // 名字不一致说明包里内容被改过：以实际内容为准，不报错（旧引用会失效但不崩）
            if &stored.name != declared {
                tracing::warn!(declared = %declared, actual = %stored.name, "存档包内资产名与内容不符，已按内容重算");
            }
            n += 1;
        }
        Ok(n)
    }
}

/// 递归收集任意 JSON 里所有 `{"asset": "<name>"}` 引用。
/// 与实体类型解耦：以后新增图片字段（地点插图、物品图标…）无需改这里。
pub fn collect_asset_refs(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(name)) = map.get("asset") {
                if is_valid_asset_name(name) {
                    out.insert(name.clone());
                }
            }
            for v in map.values() {
                collect_asset_refs(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_asset_refs(v, out);
            }
        }
        _ => {}
    }
}

/// 收集一个存档包引用的全部资产
fn bundle_refs(pkg: &SavePackage) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    collect_asset_refs(&pkg.save.storybook, &mut refs);
    for c in &pkg.commands {
        collect_asset_refs(&c.payload, &mut refs);
    }
    for c in &pkg.archived_commands {
        collect_asset_refs(&c.payload, &mut refs);
    }
    refs
}

/// 打包为 zip：`<manifest_name>` + `assets/<name>`。
/// 资产本身已是压缩格式，用 Stored 直存（更快，且不再膨胀）。
/// 存档包与故事书包共用这一份实现，只有清单条目名不同。
fn write_bundle(
    manifest_name: &str,
    manifest: &[u8],
    refs: &BTreeSet<String>,
    assets: &AssetStore,
) -> Result<Vec<u8>, EngineError> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        zip.start_file(manifest_name, opts)
            .map_err(|e| EngineError::Internal(format!("打包清单失败: {e}")))?;
        zip.write_all(manifest)
            .map_err(|e| EngineError::Internal(format!("写入清单失败: {e}")))?;

        for name in refs {
            let Some(bytes) = assets.read_blocking(name) else {
                // 缺图不致命：JSON 里的引用会指向 404，界面回落到占位
                tracing::warn!(asset = %name, "包内引用的资产不在库中，已跳过");
                continue;
            };
            zip.start_file(format!("{BUNDLE_ASSET_DIR}{name}"), opts)
                .map_err(|e| EngineError::Internal(format!("打包资产失败: {e}")))?;
            zip.write_all(&bytes)
                .map_err(|e| EngineError::Internal(format!("写入资产失败: {e}")))?;
        }
        zip.finish()
            .map_err(|e| EngineError::Internal(format!("收尾 zip 包失败: {e}")))?;
    }
    Ok(buf)
}

/// 解开 zip 包：返回（清单文本，资产）。`what` = 包的称呼，只用于报错措辞。
fn read_bundle(
    bytes: &[u8],
    manifest_name: &str,
    what: &str,
) -> Result<(String, Vec<(String, Vec<u8>)>), EngineError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| EngineError::InvalidAsset(format!("{what}不是有效的 zip: {e}")))?;

    let mut manifest: Option<String> = None;
    let mut assets: Vec<(String, Vec<u8>)> = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| EngineError::InvalidAsset(format!("读取{what}条目失败: {e}")))?;
        let entry_name = entry.name().to_string();
        if entry_name == manifest_name {
            let mut s = String::new();
            entry
                .read_to_string(&mut s)
                .map_err(|e| EngineError::InvalidAsset(format!("读取{what}清单失败: {e}")))?;
            manifest = Some(s);
        } else if let Some(asset) = entry_name.strip_prefix(BUNDLE_ASSET_DIR) {
            if !is_valid_asset_name(asset) {
                continue;
            }
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|e| EngineError::InvalidAsset(format!("读取资产失败: {e}")))?;
            assets.push((asset.to_string(), data));
        }
    }

    let manifest = manifest.ok_or_else(|| {
        EngineError::InvalidAsset(format!("{what}缺少 {manifest_name}（可能不是 octopus 包）"))
    })?;
    Ok((manifest, assets))
}

/// 打包存档为 zip：`save.json` + `assets/<name>`。
pub fn pack_bundle(pkg: &SavePackage, assets: &AssetStore) -> Result<Vec<u8>, EngineError> {
    let manifest = serde_json::to_vec_pretty(pkg)?;
    write_bundle(BUNDLE_MANIFEST, &manifest, &bundle_refs(pkg), assets)
}

/// 解开存档包。兼容两种输入：
/// - **zip**（当前格式，含 `save.json` + `assets/`）
/// - **旧版单 JSON**（历史导出的 `.octopus.json`，不含资产）
pub fn unpack_bundle(bytes: &[u8]) -> Result<(SavePackage, Vec<(String, Vec<u8>)>), EngineError> {
    if bytes.first() == Some(&b'{') {
        let pkg: SavePackage = serde_json::from_slice(bytes)
            .map_err(|e| EngineError::InvalidAsset(format!("存档 JSON 解析失败: {e}")))?;
        return Ok((pkg, Vec::new()));
    }

    let (manifest, assets) = read_bundle(bytes, BUNDLE_MANIFEST, "存档包")?;
    let pkg: SavePackage = serde_json::from_str(&manifest)
        .map_err(|e| EngineError::InvalidAsset(format!("存档清单解析失败: {e}")))?;
    Ok((pkg, assets))
}

/// 收集一本故事书包引用的全部资产：草稿 + 已发布快照（封面 / 立绘 / 插图 / 图标都在这两棵树里）。
fn book_refs(pkg: &StorybookPackage) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    collect_asset_refs(&pkg.storybook.draft, &mut refs);
    if let Some(released) = &pkg.storybook.released {
        collect_asset_refs(released, &mut refs);
    }
    refs
}

/// 打包故事书为 zip：`storybook.json` + `assets/<name>`。
/// 「都导出」：草稿、已发布版次快照、以及两者引用的全部图片。
pub fn pack_book_bundle(pkg: &StorybookPackage, assets: &AssetStore) -> Result<Vec<u8>, EngineError> {
    let manifest = serde_json::to_vec_pretty(pkg)?;
    write_bundle(BOOK_BUNDLE_MANIFEST, &manifest, &book_refs(pkg), assets)
}

/// 解开故事书包（zip：`storybook.json` + `assets/`）。
pub fn unpack_book_bundle(
    bytes: &[u8],
) -> Result<(StorybookPackage, Vec<(String, Vec<u8>)>), EngineError> {
    let (manifest, assets) = read_bundle(bytes, BOOK_BUNDLE_MANIFEST, "故事书包")?;
    let pkg: StorybookPackage = serde_json::from_str(&manifest)
        .map_err(|e| EngineError::InvalidAsset(format!("故事书清单解析失败: {e}")))?;
    Ok((pkg, assets))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_root() -> PathBuf {
        std::env::temp_dir().join(format!("octopus-assets-test-{}", uuid::Uuid::new_v4()))
    }

    fn png(tag: &[u8]) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(tag);
        v
    }

    /// 注意：SaveDetail 的 item 是 #[serde(flatten)]，所以字段平铺在 save 下
    fn sample_pkg(storybook: Value) -> SavePackage {
        serde_json::from_value(json!({
            "format": "octopus-save-package",
            "version": 1,
            "exported_at": "2026-01-01T00:00:00Z",
            "save": {
                "id": "sv-test",
                "title": "测试档",
                "storybook_id": "sb-test",
                "storybook_title": "测试书",
                "embedded_revision": 1,
                "latest_revision": 1,
                "needs_upgrade": false,
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "last_played_at": "2026-01-01T00:00:00Z",
                "storybook": storybook
            },
            "commands": [],
            "archived_commands": [],
            "maintenance": []
        }))
        .expect("构造测试存档包")
    }

    #[test]
    fn rejects_traversal_and_bad_names() {
        assert!(!is_valid_asset_name("../../etc/passwd"));
        assert!(!is_valid_asset_name("abcd.png"));
        assert!(!is_valid_asset_name(&format!("{}.exe", "a".repeat(64))));
        assert!(!is_valid_asset_name(&format!("{}.png", "A".repeat(64))));
        assert!(is_valid_asset_name(&format!("{}.webp", "a".repeat(64))));
    }

    #[test]
    fn sniff_and_content_type() {
        assert_eq!(sniff_ext(&png(b"x")), Some("png"));
        assert_eq!(sniff_ext(b"nope"), None);
        assert_eq!(content_type_of("aa.webp"), "image/webp");
        assert_eq!(content_type_of("aa.bin"), "application/octet-stream");
    }

    #[test]
    fn collects_nested_refs_and_ignores_junk() {
        let sb = json!({
            "meta": { "cover": { "asset": format!("{}.png", "b".repeat(64)), "w": 10, "h": 10 } },
            "characters": [ { "portrait": { "asset": format!("{}.webp", "c".repeat(64)), "w": 1, "h": 1 } } ],
            "junk": { "asset": "not-a-hash.png" }
        });
        let mut refs = BTreeSet::new();
        collect_asset_refs(&sb, &mut refs);
        assert_eq!(refs.len(), 2);
    }

    #[tokio::test]
    async fn put_dedups_and_reads_back() {
        let store = AssetStore::open(tmp_root()).await.unwrap();
        let a = store.put(&png(b"one")).await.unwrap();
        let b = store.put(&png(b"one")).await.unwrap();
        let c = store.put(&png(b"two")).await.unwrap();
        assert_eq!(a.name, b.name, "同内容应命中同一资产");
        assert_ne!(a.name, c.name);
        assert_eq!(store.read(&a.name).await.unwrap(), png(b"one"));
        assert!(store.put(b"not an image").await.is_err());
    }

    #[tokio::test]
    async fn bundle_round_trip_carries_assets() {
        let store = AssetStore::open(tmp_root()).await.unwrap();
        let asset = store.put(&png(b"cover")).await.unwrap();

        let pkg = sample_pkg(json!({ "meta": { "cover": { "asset": asset.name, "w": 8, "h": 8 } } }));
        let zip_bytes = pack_bundle(&pkg, &store).unwrap();

        let names: Vec<String> = {
            let mut ar = zip::ZipArchive::new(Cursor::new(&zip_bytes)).unwrap();
            (0..ar.len())
                .map(|i| ar.by_index(i).unwrap().name().to_string())
                .collect()
        };
        assert!(names.iter().any(|n| n == BUNDLE_MANIFEST));
        assert!(names.iter().any(|n| n == &format!("{BUNDLE_ASSET_DIR}{}", asset.name)));

        let (back, assets) = unpack_bundle(&zip_bytes).unwrap();
        assert_eq!(back.save.item.id, "sv-test");
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].0, asset.name);
        assert_eq!(assets[0].1, png(b"cover"));

        // 解到另一个空库：ingest 后可直接读出（跨库迁移的关键一步）
        let store2 = AssetStore::open(tmp_root()).await.unwrap();
        store2.ingest(&assets).await.unwrap();
        assert_eq!(store2.read(&asset.name).await.unwrap(), png(b"cover"));
    }

    /// 一本最小故事书包：草稿与已发布快照同源（都指向同一张封面）。
    fn sample_book_pkg(storybook: Value) -> StorybookPackage {
        serde_json::from_value(json!({
            "format": "octopus-storybook-package",
            "version": 1,
            "exported_at": "2026-01-01T00:00:00Z",
            "storybook": {
                "id": "sb-test",
                "title": "测试书",
                "revision": 2,
                "updated_at": "2026-01-01T00:00:00Z",
                "released_at": "2026-01-01T00:00:00Z",
                "published": true,
                "draft": storybook.clone(),
                "released": storybook
            }
        }))
        .expect("构造测试故事书包")
    }

    fn zip_entry_names(bytes: &[u8]) -> Vec<String> {
        let mut ar = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..ar.len())
            .map(|i| ar.by_index(i).unwrap().name().to_string())
            .collect()
    }

    #[tokio::test]
    async fn book_bundle_round_trip_carries_assets() {
        let store = AssetStore::open(tmp_root()).await.unwrap();
        let cover = store.put(&png(b"cover")).await.unwrap();
        let portrait = store.put(&png(b"portrait")).await.unwrap();

        // 封面进草稿、立绘进已发布快照：两边引用的图都必须进包
        let pkg = sample_book_pkg(json!({
            "meta": { "id": "sb-test", "cover": { "asset": cover.name, "w": 8, "h": 8 } },
            "characters": [ { "portrait": { "asset": portrait.name, "w": 4, "h": 4 } } ]
        }));
        let zip_bytes = pack_book_bundle(&pkg, &store).unwrap();

        let names = zip_entry_names(&zip_bytes);
        assert!(names.iter().any(|n| n == BOOK_BUNDLE_MANIFEST));
        assert!(names
            .iter()
            .any(|n| n == &format!("{BUNDLE_ASSET_DIR}{}", cover.name)));
        assert!(names
            .iter()
            .any(|n| n == &format!("{BUNDLE_ASSET_DIR}{}", portrait.name)));

        let (back, assets) = unpack_book_bundle(&zip_bytes).unwrap();
        assert_eq!(back.storybook.id, "sb-test");
        assert_eq!(back.storybook.revision, 2);
        assert!(back.storybook.published);
        assert_eq!(assets.len(), 2);

        // 解到另一个空库：ingest 后立绘与封面都能直接读回
        let store2 = AssetStore::open(tmp_root()).await.unwrap();
        store2.ingest(&assets).await.unwrap();
        assert_eq!(store2.read(&portrait.name).await.unwrap(), png(b"portrait"));
    }

    #[tokio::test]
    async fn book_and_save_bundles_do_not_masquerade() {
        let store = AssetStore::open(tmp_root()).await.unwrap();
        // 存档包（save.json）拿给故事书解包器 → 明确报错，不会解析出半个对象
        let save_zip = pack_bundle(&sample_pkg(json!({})), &store).unwrap();
        let err = unpack_book_bundle(&save_zip).unwrap_err().to_string();
        assert!(err.contains(BOOK_BUNDLE_MANIFEST), "报错要指出缺的是故事书清单: {err}");

        // 反向同理
        let book_zip = pack_book_bundle(&sample_book_pkg(json!({})), &store).unwrap();
        let err = unpack_bundle(&book_zip).unwrap_err().to_string();
        assert!(err.contains(BUNDLE_MANIFEST), "报错要指出缺的是存档清单: {err}");
    }

    #[test]
    fn unpack_accepts_legacy_single_json() {
        let bytes = serde_json::to_vec(&sample_pkg(json!({}))).unwrap();
        let (back, assets) = unpack_bundle(&bytes).unwrap();
        assert_eq!(back.save.item.id, "sv-test");
        assert!(assets.is_empty(), "旧版单 JSON 不含资产");
    }
}
