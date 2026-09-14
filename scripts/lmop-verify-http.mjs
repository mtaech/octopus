#!/usr/bin/env node
// LMoP 故事书「真实 HTTP 路径」验证（不需要 AI，确定性）。
//
// 走真实产品 API：创建故事书 -> 存草稿 -> 发布 -> 开档 -> 读存档投影。
// 断言（验收 1/2/4 的 HTTP 侧）：
//   1. 基于 story_example/lmop-storybook.json 创建并发布成功
//   2. 开档后的世界投影不含 kind="monster" 的初始实例
//   4. 受控角色是 kind="pc" 的角色；locations 存在；PC location_id 来自模板常驻地
//      （maps 只在故事书里，投影不下发 —— 脚本分别断言）
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const BASE = (() => {
  const i = process.argv.indexOf('--base');
  return i >= 0 ? process.argv[i + 1] : 'http://127.0.0.1:8787';
})();
const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SB_PATH = join(ROOT, 'story_example/lmop-storybook.json');

let pass = 0, fail = 0;
function check(name, ok, evidence) {
  if (ok) { pass++; console.log('[PASS] ' + name); }
  else { fail++; console.log('[FAIL] ' + name); }
  if (evidence !== undefined) console.log('       ' + String(evidence).slice(0, 900));
}

let token = '';
async function api(method, path, body) {
  const res = await fetch(BASE + path, {
    method,
    headers: Object.assign(
      { 'Content-Type': 'application/json' },
      token ? { Authorization: 'Bearer ' + token } : {},
    ),
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json = null;
  try { json = JSON.parse(text); } catch {}
  return { status: res.status, text, json };
}
function snippet(o, n) { const s = o === undefined ? 'undefined' : JSON.stringify(o); return String(s).slice(0, n || 400); }

const raw = readFileSync(SB_PATH, 'utf8');
const sb = JSON.parse(raw);
console.log('== LMoP 真实 HTTP 路径验证 ==');
console.log('base =', BASE);
console.log('storybook =', SB_PATH, '(' + (raw.length / 1024).toFixed(0) + ' KB)');

// 0) 登录
const login = await api('POST', '/api/auth/login', { username: 'octopus', password: 'octopus' });
console.log('\n[req] POST /api/auth/login -> ' + login.status + ' ' + snippet(login.json));
if (login.status !== 200) { console.error('登录失败，无法继续'); process.exit(2); }
token = login.json.token;

// 1) 创建故事书
const title = '凡戴尔的失落矿坑（验证 ' + new Date().toISOString().slice(0, 19) + '）';
const created = await api('POST', '/api/storybooks', { title });
console.log('\n[req] POST /api/storybooks -> ' + created.status + ' ' + snippet(created.json));
check('1a 创建故事书（POST /api/storybooks）返回 201', created.status === 201, snippet(created.json));
if (created.status !== 201) process.exit(3);
const sbId = created.json.id;
const draftVersion0 = created.json.draft_version;

// 2) 存草稿（meta.id / meta.title 必须与故事书身份一致）
sb.meta = Object.assign({}, sb.meta || {}, { id: sbId, title });
const saved = await api('PUT', '/api/storybooks/' + sbId, { draft: sb, base_version: draftVersion0 });
console.log('\n[req] PUT /api/storybooks/' + sbId + ' -> ' + saved.status + ' ' + snippet(saved.json, 300));
check('1b 存入 LMoP 草稿成功', saved.status === 200, snippet(saved.json, 300));
if (saved.status !== 200) process.exit(4);

// 3) 发布
const pub = await api('POST', '/api/storybooks/' + sbId + '/publish', { base_version: saved.json.doc.draft_version });
console.log('\n[req] POST /api/storybooks/' + sbId + '/publish -> ' + pub.status + ' ' + snippet(pub.json, 800));
check('1c 发布成功且 revision=1', pub.status === 200 && pub.json && pub.json.doc && pub.json.doc.revision === 1, snippet(pub.json, 800));
if (pub.status !== 200) process.exit(5);

// 4) 开档（基于已发布版次）
const save = await api('POST', '/api/saves', { storybook_id: sbId, title: null, controlled_character_id: null, is_sandbox: null });
console.log('\n[req] POST /api/saves -> ' + save.status + ' ' + snippet({ id: save.json && save.json.id, embedded_revision: save.json && save.json.embedded_revision }, 300));
check('1d 开档成功（201）且版次=1', save.status === 201 && save.json && save.json.embedded_revision === 1, 'status=' + save.status + ' id=' + (save.json && save.json.id) + ' rev=' + (save.json && save.json.embedded_revision));
if (save.status !== 201) process.exit(6);
const saveId = save.json.id;

// 5) 读存档投影
const state = await api('GET', '/api/saves/' + saveId + '/state');
console.log('\n[req] GET /api/saves/' + saveId + '/state -> ' + state.status);
const proj = state.json;
console.log('       projection keys =', Object.keys(proj || {}).join(','));
console.log('       controlled =', JSON.stringify(proj && proj.controlled));
console.log('       scene_id =', proj && proj.scene_id);
console.log('       characters =', Object.keys((proj && proj.characters) || {}).join(','));
console.log('       locations =', ((proj && proj.locations) || []).length);

const chars = (proj && proj.characters) || {};
const kinds = {};
for (const [k, v] of Object.entries(chars)) kinds[v.kind] = (kinds[v.kind] || 0) + 1;
console.log('       character kinds =', JSON.stringify(kinds));

check('2 投影不含任何 kind="monster" 的初始实例', !Object.values(chars).some((c) => c.kind === 'monster'),
  'kinds=' + JSON.stringify(kinds) + ' keys=' + Object.keys(chars).join(','));
check('2b 初始实例表只含 PC 一个（图鉴是模板库）', Object.keys(chars).length === 1, 'keys=' + Object.keys(chars).join(','));

const pcKey = ((proj && proj.controlled) || [])[0];
const pc = pcKey ? chars[pcKey] : null;
check('4a 受控角色是模板里的 kind="pc" 角色（塔林·银溪）',
  !!pc && pc.kind === 'pc' && pc.template_id === 'pc-lmop-talin', snippet(pc, 500));
check('4b PC 的 location_id 来自模板常驻地 loc-lmop-040（凡达林）',
  pc && pc.location_id === 'loc-lmop-040', 'location_id=' + (pc && pc.location_id));
check('4c 投影 locations 存在且为 88 个地点', Array.isArray(proj && proj.locations) && proj.locations.length === 88,
  'locations=' + (((proj && proj.locations) || []).length));

// 6) maps：投影不下发，只在故事书里（用存档详情读回）
const detail = await api('GET', '/api/saves/' + saveId);
const maps = (detail.json && detail.json.storybook && detail.json.storybook.world && detail.json.storybook.world.maps) || [];
console.log('\n[req] GET /api/saves/' + saveId + ' -> ' + detail.status + '; world.maps = ' + maps.length);
const pinCount = maps.reduce((a, m) => a + (m.pins || []).length, 0);
const badPin = maps.flatMap((m) => (m.pins || []).filter((p) => !(p.x >= 0 && p.x <= 1 && p.y >= 0 && p.y <= 1)));
check('4d 存档内 world.maps 存在（7 张 / 93 锚点）且 pin 坐标全在 0..1',
  maps.length === 7 && pinCount === 93 && badPin.length === 0,
  'maps=' + maps.length + ' pins=' + pinCount + ' 越界=' + badPin.length);
check('4e 世界投影本身不含 maps 字段（如实记录：地图只在故事书层）',
  !('maps' in (proj || {})), 'projection keys=' + Object.keys(proj || {}).join(','));

console.log('\n== 结论：' + pass + ' 通过 / ' + fail + ' 未通过 ==');
console.log(JSON.stringify({ storybook_id: sbId, save_id: saveId, pass, fail }, null, 2));
process.exit(fail === 0 ? 0 : 1);