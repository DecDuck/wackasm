pub fn count_primes(limit: u32) -> u32 {
    if limit < 2 {
        return 0;
    }

    let odd_candidates = ((limit - 1) / 2) as usize;
    let mut sieve = vec![1_u8; odd_candidates];

    for index in 0..odd_candidates {
        let prime = index as u64 * 2 + 3;
        let square = prime * prime;
        if square > u64::from(limit) {
            break;
        }
        if sieve[index] == 0 {
            continue;
        }

        let first_composite = ((square - 3) / 2) as usize;
        for composite in (first_composite..odd_candidates).step_by(prime as usize) {
            sieve[composite] = 0;
        }
    }

    1 + sieve.iter().filter(|&&is_prime| is_prime != 0).count() as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn run_prime_benchmark(limit: u32, rounds: u32) -> u64 {
    (0..rounds)
        .map(|round| u64::from(count_primes(limit.saturating_sub(round))))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::{count_primes, run_prime_benchmark};

    #[test]
    fn counts_primes_for_small_limits() {
        assert_eq!(count_primes(0), 0);
        assert_eq!(count_primes(2), 1);
        assert_eq!(count_primes(3), 2);
        assert_eq!(count_primes(10), 4);
        assert_eq!(count_primes(30), 10);
    }

    #[test]
    fn benchmark_repeats_with_decreasing_limits() {
        assert_eq!(run_prime_benchmark(10, 2), 8);
    }
}
