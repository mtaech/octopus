#!/usr/bin/env bash
# T15 引擎边界自查（docs/rules-via-lua.md §6 一句话边界）：
#   引擎实现里不应出现 advantage / proficiency / on_save / save_ends / ConditionalModifier /
#   EncounterTable / extends 这类**规则集语义**词汇；出现即说明规则集语义漏进了引擎。
# 允许的既有例外：CheckKind::Save（封闭「签名」枚举——谁掷骰 / 比较对象是什么，不含规则集语义）。
set -uo pipefail
cd "$(dirname "$0")/.."
PATTERN='advantage|proficiency|on_save|save_ends|ConditionalModifier|EncounterTable|extends'
TARGETS=(crates/octopus-engine/src crates/octopus-types/src)
echo '== T15 引擎边界自查 =='
echo "pattern = $PATTERN"
echo "targets = ${TARGETS[*]}"
hits=$(grep -rniE "$PATTERN" "${TARGETS[@]}" || true)
if [ -n "$hits" ]; then
  echo '[FAIL] 命中规则集词汇：'
  echo "$hits"
  exit 1
fi
echo '[PASS] 禁词零命中（engine/src 与 types/src）'
echo
echo '-- 既有例外（封闭签名，允许）--'
grep -rn 'CheckKind::Save\|CheckKind {' crates/octopus-types/src/lib.rs | head -5 || true
grep -rn 'Save,' crates/octopus-types/src/lib.rs | head -3 || true
echo
echo '-- 对照：同一批词在规则包故事书里是**必须出现**的（规则集语义归 Lua / 开放内容）--'
for w in advantage proficiency on_save save_ends ConditionalModifier EncounterTable extends; do
  n=$(grep -oiE "$w" story_example/lmop-storybook.json | wc -l)
  echo "  storybook: $w = $n"
done
exit 0