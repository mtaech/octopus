// 运行模式判定：真实后端（默认）或本地 Mock（?mock=1 / localStorage 开关）。
//
// 单独成模块的理由：`lib/auth.ts` 也要判断 Mock 模式，而 `api/index.ts` 要复用
// 同一份实现——放在这里两边都 import 它，不产生循环依赖。
export function isMockMode(): boolean {
  if (typeof window === 'undefined') return true
  const params = new URLSearchParams(window.location.search)
  if (params.get('mock') === '1') return true
  if (localStorage.getItem('octopus:force_mock') === '1') return true
  return false
}

export function setMockMode(forceMock: boolean): void {
  if (forceMock) {
    localStorage.setItem('octopus:force_mock', '1')
  } else {
    localStorage.removeItem('octopus:force_mock')
  }
  window.location.reload()
}
