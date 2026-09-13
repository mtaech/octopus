//! 确定性 RNG（#12 ④）：存档级种子 + 消费记录进命令日志，重放时直接复用记录值。
//!
//! splitmix64 的状态只由「种子 + 已消耗次数」决定：每次取值固定加一个常数并重新混淆，
//! 因此只要知道消耗次数，就能在任何进程里逐位复现同一条序列。这是重放 / 快照恢复
//! （#06 ②）无需把内部状态整体序列化的依据——日志里记下每段消耗的结果条数即可。

#[derive(Debug, Clone)]
pub struct DeterministicRng {
    /// 初始状态（供复位到任意消耗位置用）。
    seed: u64,
    state: u64,
    /// 每次取值的原始输出，按顺序记录（进命令日志 rng_consume）。
    pub consumed: Vec<u64>,
}

/// splitmix64 的固定自增步长（new 与 restore 必须一致）。
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        Self { seed, state: seed ^ GAMMA, consumed: Vec::new() }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let out = z ^ (z >> 31);
        self.consumed.push(out);
        out
    }

    /// 闭区间 [min, max]；max <= min 时返回 min 且不消耗。
    pub fn range_inclusive(&mut self, min: i64, max: i64) -> i64 {
        if max <= min {
            return min;
        }
        let span = (max - min + 1) as u64;
        min + (self.next_u64() % span) as i64
    }

    /// 已消耗的取值次数 = RNG 当前位置（命令日志 rng_consume 累计条数）。
    pub fn position(&self) -> usize {
        self.consumed.len()
    }

    /// 复位到「同种子下已消耗 position 次」的状态。
    ///
    /// 重放 / 载入快照 / 回滚后都据此把 RNG 拨回日志记录的位置：先清空进度、从种子重跑
    /// 相同次数。因为第 N 次输出只取决于 N，重跑得到的 consumed 与实时会话逐位一致。
    pub fn restore(&mut self, position: usize) {
        self.state = self.seed ^ GAMMA;
        self.consumed.clear();
        while self.consumed.len() < position {
            self.next_u64();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DeterministicRng;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = DeterministicRng::new(42);
        let mut b = DeterministicRng::new(42);
        let va: Vec<i64> = (0..16).map(|_| a.range_inclusive(1, 20)).collect();
        let vb: Vec<i64> = (0..16).map(|_| b.range_inclusive(1, 20)).collect();
        assert_eq!(va, vb, "同种子必须同序列（可重放前提）");
        assert!(va.iter().all(|v| (1..=20).contains(v)));
    }

    #[test]
    fn consumed_log_records_every_draw() {
        let mut r = DeterministicRng::new(7);
        let _ = r.range_inclusive(1, 6);
        let _ = r.range_inclusive(1, 100);
        assert_eq!(r.consumed.len(), 2);
    }

    /// 位置可报告、可复位：复位到 N 次后再取的值，与不中断连续取到 N 次后的下一个值相同。
    #[test]
    fn restore_reproduces_the_same_continuation() {
        let mut reference = DeterministicRng::new(2024);
        for _ in 0..5 {
            let _ = reference.range_inclusive(1, 20);
        }
        let expected_next = reference.range_inclusive(1, 20);

        let mut restored = DeterministicRng::new(2024);
        restored.restore(5);
        assert_eq!(restored.position(), 5);
        assert_eq!(restored.consumed, reference.consumed[..5].to_vec());
        let next = restored.range_inclusive(1, 20);
        assert_eq!(next, expected_next, "复位后未来的取值必须与不断线一致");
    }
}
