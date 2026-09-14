// A 工作台选中态（模块级共享）
// 用对象身份生成稳定键：用户改写实体 id 时选中不丢；实体被删除时键失效，面板自动回落。
import { computed, ref, toRaw, type WritableComputedRef } from 'vue'

const selection = ref<Record<string, string>>({})

/** 每个 tab 一份选中态，切 tab 回来仍停在上次编辑的实体 */
export function useWorkbenchSelection(tab: string): WritableComputedRef<string> {
  return computed({
    get: () => selection.value[tab] ?? '',
    set: (v: string) => { selection.value[tab] = v },
  })
}

let seq = 0
const keys = new WeakMap<object, string>()

/** 实体 → 稳定键（与可编辑的 id 字段解耦）。
 *  必须先 toRaw：同一实体的「原始对象」与「响应式代理」是两个不同的 WeakMap 键，
 *  不归一就会出现「刚新增的实体立刻掉选中、回落到清单第一条」。 */
export function entityKey(obj: object): string {
  const raw = toRaw(obj)
  let k = keys.get(raw)
  if (!k) { k = 'e' + (++seq); keys.set(raw, k) }
  return k
}
