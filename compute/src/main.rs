use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let limit = arguments
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(20_000_000);
    let rounds = arguments
        .next()
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(10);

    let started = Instant::now();
    let checksum = compute::run_prime_benchmark(limit, rounds);
    println!(
        "prime benchmark: limit={limit}, rounds={rounds}, checksum={checksum}, elapsed={:?}",
        started.elapsed()
    );

    Ok(())
}
