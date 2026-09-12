/** 游玩页左栏可选中的世界对象；右栏据此显示详情。 */
export type WorldSelection =
  | { kind: 'goal'; id: string }
  | { kind: 'trigger'; id: string }
  | { kind: 'location'; id: string }
  | { kind: 'character'; id: string }
  | { kind: 'encounter'; id: string }
