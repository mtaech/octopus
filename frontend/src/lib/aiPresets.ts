import type { RoleConfig } from '@/types'

// 采样预设档：借鉴 SillyTavern 预设指南的「四套即用方案」。
// 只覆盖采样参数（不动模型 / 供应商），是「调参比换模型更管用」的落点。
export interface AiPreset {
  name: string
  summary: string
  values: Pick<RoleConfig, 'temperature' | 'top_p' | 'presence_penalty' | 'frequency_penalty' | 'repetition_penalty' | 'stop'>
}

export const AI_PRESETS: AiPreset[] = [
  {
    name: '创意写作',
    summary: '高创造力、低抑制：诗歌 / 脑洞 / 小说续写',
    values: { temperature: 1.2, top_p: 0.95, frequency_penalty: 0 },
  },
  {
    name: '日常 RP',
    summary: '创造力与连贯性平衡，万能方案',
    values: { temperature: 0.9, top_p: 0.9, frequency_penalty: 0.1 },
  },
  {
    name: '严肃对话',
    summary: '精确稳定：深度对话 / 技术讨论 / 推演',
    values: { temperature: 0.6, top_p: 0.85, frequency_penalty: 0.1 },
  },
  {
    name: 'NPC 模式',
    summary: '人格稳定输出：世界观解说 / 常态 NPC',
    values: { temperature: 0.7, top_p: 0.9, frequency_penalty: 0.1 },
  },
]

/** 套用预设：写采样参数并留下 preset 标签（供 UI 回显）。 */
export function applyPreset(role: RoleConfig, preset: AiPreset): void {
  role.temperature = preset.values.temperature
  role.top_p = preset.values.top_p
  role.presence_penalty = preset.values.presence_penalty
  role.frequency_penalty = preset.values.frequency_penalty
  role.repetition_penalty = preset.values.repetition_penalty
  role.stop = preset.values.stop
  role.preset = preset.name
}
