#!/usr/bin/env node
// 第三轮 · LMoP 故事书「真实 HTTP 路径」独立验证（不需要 AI，确定性）。
//
// 走真实产品 API（AGENTS.md 指定起服务方式 ./start.sh backend）：
//   POST /api/auth/login -> POST /api/storybooks -> PUT /api/storybooks/{id}
//   -> POST /api/storybooks/{id}/publish -> POST /api/saves
//   -> GET /api/saves/{id}/state（投影） -> GET /api/saves/{id}（存档故事书）
//
// 断言（任务清单 1 / 2 + 附加）：
//   1. 用**重新生成后**的 story_example/lmop-storybook.json 建书 + 发布 + 开档成功
//   2. 受控角色就是故事书里唯一那个 kind="pc" 的角色；投影**不含** kind="monster" 初始实例
//   3. locations 与故事书同构；maps 锚点坐标合法（并如实记录投影层有没有 maps）
//   4. 只新增验证数据：跑前跑后列出故事书 / 存档清单，确认既有条目**一个都没少**
//
// 注意：真实库 octopus.db 里有用户自己的数据。本脚本只做「新增」，绝不删除 / 修改既有条目。
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const BASE = (() => {
  const i = process.argv.indexOf('--base');
  return i >= 0 ? process.argv[i + 1] : 'http://127.0.0.1:8787';
})();
const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SB_PATH = join(ROOT, 'story_example/lmop-storybook.json');

let pass = 0, fail = 0;
const failures = [];
function check(name, ok, evidence) {
  if (ok) { pass++; console.log('[PASS] ' + name); }
  else { fail++; failures.push(name); console.log('[FAIL] ' + name); }
  if (evidence !== undefined) console.log('       ' + String(evidence).slice(0, 700));
}
function snip(o, n) { return String(o === undefined ? 'undefined' : JSON.stringify(o)).slice(0, n || 400); }

let token = '';
async function api(method, path, body) {
  const res = await fetch(BASE + path, {
    method,
    headers: Object.assign({ 'Content-Type': 'application/json' }, token ? { Authorization: 'Bearer ' + token } : {}),
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json = null; try { json = JSON.parse(text); } catch {}
  return { status: res.status, text, json };
}

const raw = readFileSync(SB_PATH, 'utf8');
const sha = createHash('sha256').update(raw).digest('hex');
const sb = JSON.parse(raw);
console.log('== 第三轮 LMoP 真实 HTTP 路径验证 ==');
console.log('base =', BASE);
console.log('storybook =', SB_PATH, '(' + (raw.length / 1024).toFixed(0) + ' KB)');
console.log('sha256 =', sha);

// ---- 登录：优先默认管理员；口令被改过时走真实注册端点开一个验证账户 ----
let login = await api('POST', '/api/auth/login', { username: 'octopus', password: 'octopus' });
console.log('\n[req] POST /api/auth/login (octopus) -> ' + login.status + ' ' + snip(login.json));
let account = 'octopus';
if (login.status !== 200) {
  const uname = 'verify3-' + Date.now().toString(36);
  const reg = await api('POST', '/api/auth/register', { username: uname, password: uname + '-pass-123' });
  console.log('[req] POST /api/auth/register (' + uname + ') -> ' + reg.status + ' ' + snip(reg.json));
  check('0 默认管理员口令不可用，真实注册端点可用（验证账户 ' + uname + '）', reg.status === 201, snip(reg.json, 300));
  if (reg.status !== 201) process.exit(2);
  login = await api('POST', '/api/auth/login', { username: uname, password: uname + '-pass-123' });
  console.log('[req] POST /api/auth/login (' + uname + ') -> ' + login.status);
  account = uname;
}
if (login.status !== 200) { console.error('登录失败'); process.exit(2); }
token = login.json.token;

// ---- 跑前快照（证明不动既有数据）----
const beforeBooks = await api('GET', '/api/storybooks');
const beforeSaves = await api('GET', '/api/saves');
const beforeBookIds = new Set((beforeBooks.json || []).map((b) => b.id));
const beforeSaveIds = new Set((beforeSaves.json || []).map((s) => s.id));
console.log('\n[req] GET /api/storybooks -> ' + beforeBooks.status + ' 既有 ' + beforeBookIds.size + ' 本：' + [...beforeBookIds].join(','));
console.log('[req] GET /api/saves      -> ' + beforeSaves.status + ' 既有 ' + beforeSaveIds.size + " 个：" + [...beforeSaveIds].join(','));

const title = '凡戴尔的失落矿坑（第三轮验证 ' + new Date().toISOString().slice(0, 19) + '）';
const created = await api('POST', '/api/storybooks', { title });
console.log('\n[req] POST /api/storybooks -> ' + created.status + ' ' + snip(created.json));
check('1a 创建故事书 201', created.status === 201, snip(created.json));
if (created.status !== 201) process.exit(3);
const sbId = created.json.id;

sb.meta = Object.assign({}, sb.meta || {}, { id: sbId, title });
const saved = await api('PUT', '/api/storybooks/' + sbId, { draft: sb, base_version: created.json.draft_version });
console.log('[req] PUT /api/storybooks/' + sbId + ' -> ' + saved.status + ' ' + snip(saved.json, 200));
check('1b 存入重新生成后的 LMoP 草稿 200', saved.status === 200, snip(saved.json, 300));
if (saved.status !== 200) process.exit(4);

const pub = await api('POST', '/api/storybooks/' + sbId + '/publish', { base_version: saved.json.doc.draft_version });
console.log('[req] POST /publish -> ' + pub.status + ' revision=' + (pub.json && pub.json.doc && pub.json.doc.revision) + ' errors=' + snip(pub.json && pub.json.errors, 300));
check('1c 发布成功且 revision=1', pub.status === 200 && pub.json.doc.revision === 1, snip(pub.json, 500));
if (pub.status !== 200) process.exit(5);

const save = await api('POST', '/api/saves', { storybook_id: sbId, title: null, controlled_character_id: null, is_sandbox: null });
console.log('[req] POST /api/saves -> ' + save.status + ' ' + snip({ id: save.json && save.json.id, embedded_revision: save.json && save.json.embedded_revision }));
check('1d 开档 201 且 embedded_revision=1',
  save.status === 201 && save.json.embedded_revision === 1,
  'status=' + save.status + ' id=' + (save.json && save.json.id) + ' rev=' + (save.json && save.json.embedded_revision));
if (save.status !== 201) process.exit(6);
const saveId = save.json.id;

// ---- 投影 ----
const state = await api('GET', '/api/saves/' + saveId + '/state');
const proj = state.json || {};
console.log('\n[req] GET /api/saves/' + saveId + '/state -> ' + state.status);
console.log('       projection keys = ' + Object.keys(proj).join(','));
const chars = proj.characters || {};
const kinds = {};
for (const v of Object.values(chars)) kinds[v.kind] = (kinds[v.kind] || 0) + 1;
console.log('       character kinds = ' + JSON.stringify(kinds));
console.log('       controlled = ' + JSON.stringify(proj.controlled));

check('2a 投影不含任何 kind="monster" 的初始实例',
  !Object.values(chars).some((c) => c.kind === 'monster'),
  'kinds=' + JSON.stringify(kinds) + ' keys=' + Object.keys(chars).join(','));
check('2b 初始实例表恰好 1 个角色', Object.keys(chars).length === 1, 'keys=' + Object.keys(chars).join(','));
check('2c 投影 encounters 初始为空', Array.isArray(proj.encounters) && proj.encounters.length === 0, 'encounters=' + snip(proj.encounters));

const pcKey = (proj.controlled || [])[0];
const pc = pcKey ? chars[pcKey] : null;
const pcTemplates = sb.characters.filter((c) => c.kind === 'pc');
check('2d 受控角色就是故事书里唯一那个 kind="pc" 的角色',
  !!pc && pc.kind === 'pc' && pcTemplates.length === 1 && pc.template_id === pcTemplates[0].id,
  'controlled=' + pcKey + ' pc=' + snip(pc, 300) + ' pc_templates=' + pcTemplates.map((c) => c.id).join(','));

check('3a 投影 locations 与故事书地点数一致（' + sb.world.locations.length + '）',
  Array.isArray(proj.locations) && proj.locations.length === sb.world.locations.length,
  'projection locations=' + ((proj.locations || []).length));
check('3b 投影**不含** maps 字段（按设计：地图只在故事书层，如实记录）', !('maps' in proj),
  'projection keys = ' + Object.keys(proj).join(',') + ' | has maps = ' + ('maps' in proj));

// ---- 故事书层的 maps / 锚点坐标 ----
const detail = await api('GET', '/api/saves/' + saveId);
const maps = (detail.json && detail.json.storybook && detail.json.storybook.world && detail.json.storybook.world.maps) || [];
console.log('\n[req] GET /api/saves/' + saveId + ' -> ' + detail.status + '; world.maps = ' + maps.length);
const pins = maps.flatMap((m) => m.pins || []);
const badPin = pins.filter((p) => !(typeof p.x === 'number' && p.x >= 0 && p.x <= 1 && typeof p.y === 'number' && p.y >= 0 && p.y <= 1));
const locIds = new Set(sb.world.locations.map((l) => l.id));
const badPinLoc = pins.filter((p) => !locIds.has(p.location_id));
check('3c maps 与锚点坐标合法（坐标 0..1、location_id 全部命中地点表）',
  maps.length > 0 && pins.length > 0 && badPin.length === 0 && badPinLoc.length === 0,
  'maps=' + maps.length + ' pins=' + pins.length + ' 越界=' + badPin.length + ' 悬空 location_id=' + badPinLoc.length);

// ---- 跑后快照：既有数据一个都没少 ----
const afterBooks = await api('GET', '/api/storybooks');
const afterSaves = await api('GET', '/api/saves');
const afterBookIds = new Set((afterBooks.json || []).map((b) => b.id));
const afterSaveIds = new Set((afterSaves.json || []).map((s) => s.id));
const lostBooks = [...beforeBookIds].filter((id) => !afterBookIds.has(id));
const lostSaves = [...beforeSaveIds].filter((id) => !afterSaveIds.has(id));
const newBooks = [...afterBookIds].filter((id) => !beforeBookIds.has(id));
const newSaves = [...afterSaveIds].filter((id) => !beforeSaveIds.has(id));
console.log('\n[req] GET /api/storybooks（跑后）-> ' + afterBooks.status + ' 共 ' + afterBookIds.size + ' 本；新增 ' + newBooks.join(','));
console.log('[req] GET /api/saves（跑后）      -> ' + afterSaves.status + ' 共 ' + afterSaveIds.size + ' 个；新增 ' + newSaves.join(','));
check('4 既有故事书 / 存档一条都没少（本脚本只新增）', lostBooks.length === 0 && lostSaves.length === 0,
  '丢失故事书=' + JSON.stringify(lostBooks) + ' 丢失存档=' + JSON.stringify(lostSaves));

console.log('\n== 结论：' + pass + ' 通过 / ' + fail + ' 未通过 ==');
if (failures.length) console.log('未通过：' + failures.join(' / '));
console.log(JSON.stringify({
  account,
  storybook_id: sbId,
  save_id: saveId,
  sha256: sha,
  new_storybook_ids: newBooks,
  new_save_ids: newSaves,
  pass, fail,
}, null, 2));
process.exit(fail === 0 ? 0 : 1);
