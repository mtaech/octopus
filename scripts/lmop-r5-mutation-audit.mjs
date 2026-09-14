#!/usr/bin/env node
// 第五轮验证资产：对「--check 声明的行为级兜底」做独立变异审计。
//
// 做法：以交付物 story_example/lmop-storybook.json 为基线，每次**只改一处**，
// 用真实引擎校验器 scripts/lmop-engine-check（真实 LuaHost）跑同一批规则断言，
// 检查「被 --check 声明为 behavioural 的引擎级断言」是否真的会 FAIL。
//
// 用法：node scripts/lmop-r5-mutation-audit.mjs [engine-check 二进制路径]
// 退出码：0 = 每个变异都让声明的断言失败（兜底有效）；1 = 有变异没被抓住。

import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const BIN = process.argv[2] || path.join(ROOT, '.scratch/lmop-engine-check-target/debug/lmop-engine-check')
const SB = path.join(ROOT, 'story_example/lmop-storybook.json')

function mountOf(book, id) {
  return (book.lua_mounts || []).find(m => m.id === id)
}

// 每个变异：只动一处，声明「必须变红的引擎级断言」。
const MUTATIONS = [
  {
    name: 'M1 拿掉伏击的 kind==attack 闸门（源码）',
    behavioural: ['ambusher.check_signature', 'ambusher.fail_closed_without_signature'],
    apply(book) {
      const m = mountOf(book, 'dnd-ambusher-keep-high')
      const before = m.source
      m.source = before.replace('if host.check_kind ~= "attack" then return end\n', '')
      if (m.source === before) throw new Error('M1 未改到源码')
    },
  },
  {
    name: 'M2 清空集群战术的失能名单（开放内容）',
    behavioural: ['pack_tactics.reads_world_facts'],
    apply(book) {
      const d = (book.definitions || []).find(x => x.id === 'pack-wolf')
      d.fields.incapacitated_statuses = []
    },
  },
  {
    name: 'M3 把 dnd-save-half 的 scale_effect(0.5) 注释掉（源码）',
    behavioural: ['save_half.engine_scales_own_roll'],
    apply(book) {
      const m = mountOf(book, 'dnd-save-half')
      const before = m.source
      m.source = before.replace('host.scale_effect(0.5)', '-- host.scale_effect(0.5)')
      if (m.source === before) throw new Error('M3 未改到源码')
    },
  },
  {
    name: 'M4 把伏击期望状态 id 改成 dnd-prone（只改开放内容，脚本逐字不动）',
    behavioural: ['ambusher.target_status_data_driven'],
    apply(book) {
      const d = (book.definitions || []).find(x => x.id === 'ambush-doppelganger')
      d.fields.target_status = 'dnd-prone'
    },
  },
  {
    name: 'M5 去掉日照敏感的 attribute+wis 分支（源码）',
    behavioural: ['sunlight.by_signature'],
    apply(book) {
      const m = mountOf(book, 'dnd-sunlight-sensitivity')
      const before = m.source
      m.source = before.replace("local sight = kind == 'attribute' and attribute == 'wis'", 'local sight = false')
      if (m.source === before) throw new Error('M5 未改到源码')
    },
  },
]

function runEngine(bookPath) {
  // 校验器在被抓住时会以非零码退出（rule_failures > 0）——正好是我们要观察的情形，
  // 所以不能因为退出码就抛，stdout 里始终是完整 JSON。
  const res = spawnSync(BIN, [bookPath], { encoding: 'utf8', maxBuffer: 1 << 28 })
  if (res.error) throw res.error
  const text = res.stdout || ''
  try {
    return JSON.parse(text)
  } catch (e) {
    throw new Error('校验器没有输出 JSON（exit=' + res.status + '）：' + text.slice(0, 500) + ' / stderr=' + (res.stderr || '').slice(0, 500))
  }
}

const base = JSON.parse(fs.readFileSync(SB, 'utf8'))
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'lmop-r5-mut-'))
let failed = 0

// 对照：未变异的交付物必须让全部目标断言为 true。
{
  const p = path.join(tmp, 'baseline.json')
  fs.writeFileSync(p, JSON.stringify(base))
  const res = runEngine(p)
  const names = [...new Set(MUTATIONS.flatMap(m => m.behavioural))]
  const byName = new Map(res.rule_assertions.map(i => [i.name, i]))
  for (const n of names) {
    const a = byName.get(n)
    if (!a) { console.log('[FAIL] 基线缺少断言 ' + n); failed++ }
    else if (!a.pass) { console.log('[FAIL] 基线下断言本应通过，实际失败：' + n); failed++ }
  }
  console.log('[基线] 交付物：' + names.length + ' 条目标断言全部在场且通过')
}

for (const mut of MUTATIONS) {
  const book = JSON.parse(JSON.stringify(base))
  mut.apply(book)
  const p = path.join(tmp, mut.name.replace(/[^a-zA-Z0-9]+/g, '_') + '.json')
  fs.writeFileSync(p, JSON.stringify(book))
  const res = runEngine(p)
  const byName = new Map(res.rule_assertions.map(i => [i.name, i]))
  for (const n of mut.behavioural) {
    const a = byName.get(n)
    if (!a) { console.log('[FAIL] ' + mut.name + ' → 断言缺失：' + n); failed++; continue }
    const verdict = a.pass ? '未被抓住（仍通过）' : '已变红'
    console.log('[' + (a.pass ? 'FAIL' : 'PASS') + '] ' + mut.name + ' → ' + n + ' ' + verdict)
    if (a.pass) failed++
  }
}

console.log('')
console.log(failed === 0
  ? '变异审计：全部 ' + MUTATIONS.length + ' 个变异都被声明的行为级断言抓住'
  : '变异审计：' + failed + ' 项没有被抓住（兜底无效）')
console.log('临时目录：' + tmp)
process.exitCode = failed === 0 ? 0 : 1
