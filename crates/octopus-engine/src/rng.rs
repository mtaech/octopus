//! 确定性 RNG（#12 ④）：存档级种子 + 消费记录进命令日志，重放时直接复用记录值。

#[derive(Debug, Clone)]
pub struct DeterministicRng {
    state: u64,
    /// 每次取值的原始输出，按顺序记录（进命令日志 rng_consume）。
    pub consumed: Vec<u64>,
}

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed ^ 0x9E37_79B9_7F4A_7C15, consumed: Vec::new() }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
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
}
