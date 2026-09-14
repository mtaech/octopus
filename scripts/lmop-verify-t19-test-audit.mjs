#!/usr/bin/env node
// 对 T19 两处测试改动的**变异审计**（第二轮验证者新增）。
//
// 目的：回答「把实现改坏，这条断言会不会 FAIL」——不是「测试通过了」。
// 本脚本把改后的 lmop_verification.rs::a1_a2 里那段 lua_mounts 断言**逐字复刻**成 JS，
// 然后对 story_example/lmop-storybook.json 的**内存副本**施加一组变异，逐条报告 PASS/FAIL。
//
// 期望语义：M1..M6 必须 FAIL（断言有牙齿）；M7 是残余覆盖盲区（如实登记，不是「弱化」）。
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const RAW = readFileSync(join(ROOT, 'story_example/lmop-storybook.json'), 'utf8');

/** 逐字复刻 a1_a2（改后）里的 lua_mounts 断言；返回 null = 通过，否则返回失败原因。 */
function assertMounts(sb) {
  const mounts = sb.lua_mounts;
  if (mounts.length !== 16) return '挂载点数量 != 16（实际 ' + mounts.length + '）';
  const mount_of = (id) => {
    const m = mounts.find((x) => x.id === id);
    if (!m) throw new Error('缺少关键挂载点 ' + id);
    return m;
  };
  try {
    const xp = mount_of('dnd-xp-award');
    if (xp.mount !== 'event') return 'XP 必须挂在 event 上（实际 ' + xp.mount + '）';
    if (!String(xp.source).includes('enemy_defeated')) return 'XP 规则必须按 enemy_defeated 开闸';
    const reset = mount_of('dnd-wander-reset');
    if (reset.mount !== 'event') return '掷表标记复位必须挂在 event 上（实际 ' + reset.mount + '）';
    if (!String(reset.source).includes('clear_flag')) return '掷表复位规则必须清标记';
    if (mounts.some((m) => String(m.id).startsWith('dnd-xp-day') || String(m.id).startsWith('dnd-xp-night'))) {
      return '旧的「按掷表行回补 XP」规则应已全部移除';
    }
  } catch (e) {
    return e.message;
  }
  return null;
}

/** 第一轮的旧断言（仅用于对照）。 */
function assertOldMountCount(sb) {
  return sb.lua_mounts.length === 38 ? null : '旧断言 lua_mounts.len() == 38 失败（实际 ' + sb.lua_mounts.length + '）';
}

const clone = () => JSON.parse(RAW);

const mutations = [
  {
    name: 'M0 未变异基线',
    expect: 'PASS',
    apply: (sb) => sb,
  },
  {
    name: 'M1 删除 XP 挂载点 dnd-xp-award',
    expect: 'FAIL',
    apply: (sb) => { sb.lua_mounts = sb.lua_mounts.filter((m) => m.id !== 'dnd-xp-award'); return sb; },
  },
  {
    name: 'M2 把 dnd-xp-award 从 event 挪到 turn_end',
    expect: 'FAIL',
    apply: (sb) => { sb.lua_mounts.find((m) => m.id === 'dnd-xp-award').mount = 'turn_end'; return sb; },
  },
  {
    name: 'M3 把 dnd-xp-award 的开闸条件从 enemy_defeated 改成 scene',
    expect: 'FAIL',
    apply: (sb) => { const m = sb.lua_mounts.find((x) => x.id === 'dnd-xp-award'); m.source = m.source.replaceAll('enemy_defeated', 'scene'); return sb; },
  },
  {
    name: 'M4 删除掷表复位挂载点 dnd-wander-reset',
    expect: 'FAIL',
    apply: (sb) => { sb.lua_mounts = sb.lua_mounts.filter((m) => m.id !== 'dnd-wander-reset'); return sb; },
  },
  {
    name: 'M5 把 dnd-wander-reset 的 host.clear_flag(flag) 整句换成 no-op',
    expect: 'FAIL',
    apply: (sb) => { const m = sb.lua_mounts.find((x) => x.id === 'dnd-wander-reset'); m.source = m.source.replace('host.clear_flag(flag)', "host.log('noop')"); return sb; },
  },
  {
    name: 'M6 重新塞回一条旧的按掷表行回补 XP（dnd-xp-day-1）',
    expect: 'FAIL',
    apply: (sb) => { sb.lua_mounts.push({ id: 'dnd-xp-day-1', mount: 'turn_end', source: "if host.event_name == 'encounter_cleared' then host.modify_resource(host.actor.id, 'res-xp', 75) end" }); return sb; },
  },
  {
    name: 'M6b 数量不变、但把 dnd-wander-reset 换成一条旧的按掷表行回补 XP（dnd-xp-day-1）',
    expect: 'FAIL',
    apply: (sb) => {
      sb.lua_mounts = sb.lua_mounts.filter((m) => m.id !== 'dnd-wander-reset');
      sb.lua_mounts.push({ id: 'dnd-xp-day-1', mount: 'turn_end', source: "if host.event_name == 'encounter_cleared' then host.modify_resource(host.actor.id, 'res-xp', 75) end" });
      return sb;
    },
  },
  {
    name: 'M7 残余盲区：删掉一条**未被点名**的规则挂载点（dnd-proficiency），再补一条同样合法的占位脚本',
    expect: 'PASS(盲区)',
    apply: (sb) => {
      sb.lua_mounts = sb.lua_mounts.filter((m) => m.id !== 'dnd-proficiency');
      sb.lua_mounts.push({ id: 'r2-placeholder', mount: 'check_pre_roll', source: 'return' });
      return sb;
    },
  },
  {
    name: 'M8 残余盲区：复位规则仍在，但把要清的 flag 写死成单行（clear_flag 字面量还在）',
    expect: 'PASS(盲区)',
    apply: (sb) => { const m = sb.lua_mounts.find((x) => x.id === 'dnd-wander-reset'); m.source = m.source.replace('host.clear_flag(flag)', "host.clear_flag('dnd-wander-day-1')"); return sb; },
  },
];

console.log('== T19 测试改动 · 变异审计（a1_a2 的 lua_mounts 断言）==');
let bad = 0;
for (const mut of mutations) {
  const sb = mut.apply(clone());
  const reason = assertMounts(sb);
  const got = reason === null ? 'PASS' : 'FAIL';
  const ok = (mut.expect === 'FAIL' && got === 'FAIL') || (mut.expect === 'PASS' && got === 'PASS') || (mut.expect === 'PASS(盲区)' && got === 'PASS');
  if (!ok) bad++;
  console.log(
    (ok ? '[OK]  ' : '[!!!] ') + mut.name + ' → 断言 ' + got +
    '（期望 ' + mut.expect + '）' + (reason ? ' · ' + reason : '')
  );
}

console.log('\n-- 对照：第一轮的旧断言（len()==38）在**未变异的当前交付物**上 --');
console.log('   旧断言结果 = ' + (assertOldMountCount(clone()) === null ? 'PASS' : 'FAIL：' + assertOldMountCount(clone())));

console.log('\n-- 对照：新断言在变异 M6 上是否比旧断言更早发现问题 --');
{
  const sb = mutations.find((m) => m.name.startsWith('M6')).apply(clone());
  console.log('   旧断言(len==38 且不查 id) = ' + (sb.lua_mounts.length === 17 ? 'FAIL(数量变了)' : 'PASS'));
}

console.log('\n== 结论：' + (bad === 0 ? '全部符合预期' : bad + ' 条不符合预期') + ' ==');
process.exit(bad === 0 ? 0 : 1);
