use std::time::{Duration, Instant};

use console::Term;
use eyre::{Result, WrapErr};
use indicatif::{HumanDuration, HumanFloatCount, MultiProgress, ProgressBar, ProgressStyle};

const ADDRESS_BYTES: usize = 20;
const BYTE_VALUES: f64 = 256.0;

pub struct Display {
    // Extras
    start_time: Instant,
    printed_salts: usize,

    // Progress Bars
    mp: MultiProgress,
    pb: ProgressManagers,
}

struct ProgressManagers {
    time: ProgressBar,
    speed: ProgressBar,
    target: ProgressBar,
}

impl Display {
    pub fn new() -> Result<Self> {
        let mp = MultiProgress::new();
        let pb = ProgressManagers {
            time: mp.add(ProgressBar::new_spinner()),
            speed: mp.add(ProgressBar::new_spinner()),
            target: mp.add(ProgressBar::new_spinner()),
        };

        Ok(Self {
            start_time: Instant::now(),
            printed_salts: 0,
            mp,
            pb,
        })
    }

    pub fn start(&self) -> Result<()> {
        let pb_style = ProgressStyle::with_template("{spinner:.blue} {msg}")
            .wrap_err("failed to build progress style")?
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]);

        self.pb.time.enable_steady_tick(Duration::from_millis(80));
        self.pb.time.set_style(pb_style.clone());
        self.pb.time.set_message("Loading...");

        self.pb.speed.enable_steady_tick(Duration::from_millis(80));
        self.pb.speed.set_style(pb_style.clone());
        self.pb.speed.set_message("Loading...");

        self.pb.target.enable_steady_tick(Duration::from_millis(80));
        self.pb.target.set_style(pb_style.clone());
        self.pb.target.set_message("Loading...");

        Term::stdout()
            .clear_screen()
            .wrap_err("failed to clear screen")?;
        Ok(())
    }

    pub fn update(
        &mut self,
        average_attempts_per_sec: f64,
        next_score_target: usize,
        found_salts: &[String],
    ) -> Result<()> {
        let total_runtime = self.start_time.elapsed().as_secs();

        if total_runtime != 0 {
            self.pb.time.set_message(format!(
                "Total Runtime: {}",
                HumanDuration(Duration::from_secs(total_runtime)),
            ));

            self.pb.speed.set_message(format!(
                "Speed: {:.2} million attempts per second",
                HumanFloatCount(average_attempts_per_sec / 1_000_000.0),
            ));

            self.pb.target.set_message(format!(
                "Current Target: {} zero bytes (ETA to next score: {})",
                next_score_target,
                eta_to_next_score(
                    next_score_target,
                    average_attempts_per_sec,
                    self.start_time.elapsed(),
                ),
            ));
        }

        for found_salt in found_salts.iter().skip(self.printed_salts) {
            self.mp
                .println(found_salt)
                .wrap_err("failed to print found salt")?;
        }
        self.printed_salts = found_salts.len();

        Ok(())
    }
}

fn eta_to_next_score(next_score_target: usize, attempts_per_sec: f64, elapsed: Duration) -> String {
    if attempts_per_sec <= 0.0 {
        return "calculating...".to_owned();
    }

    let Some(seconds) =
        expected_remaining_seconds_for_score(next_score_target, attempts_per_sec, elapsed)
    else {
        return "not possible".to_owned();
    };

    format!("{}", HumanDuration(clamped_duration_from_secs(seconds)))
}

fn expected_remaining_seconds_for_score(
    score: usize,
    attempts_per_sec: f64,
    _elapsed_without_hit: Duration,
) -> Option<f64> {
    // Each salt attempt is independent, so after no hit the remaining expectation is unchanged.
    expected_attempts_for_score(score).map(|expected_attempts| expected_attempts / attempts_per_sec)
}

fn expected_attempts_for_score(score: usize) -> Option<f64> {
    probability_for_at_least_zero_bytes(ADDRESS_BYTES, score).map(|probability| 1.0 / probability)
}

fn probability_for_at_least_zero_bytes(byte_width: usize, score: usize) -> Option<f64> {
    if score > byte_width {
        return None;
    }

    let zero_probability = 1.0 / BYTE_VALUES;
    let nonzero_probability = 1.0 - zero_probability;
    let probability = (score..=byte_width)
        .map(|zero_bytes| {
            combinations(byte_width, zero_bytes)
                * zero_probability.powi(zero_bytes as i32)
                * nonzero_probability.powi((byte_width - zero_bytes) as i32)
        })
        .sum::<f64>();

    Some(probability)
}

fn combinations(total: usize, chosen: usize) -> f64 {
    if chosen == 0 || chosen == total {
        return 1.0;
    }

    let chosen = chosen.min(total - chosen);
    (1..=chosen).fold(1.0, |acc, value| {
        acc * (total - chosen + value) as f64 / value as f64
    })
}

fn clamped_duration_from_secs(seconds: f64) -> Duration {
    if !seconds.is_finite() {
        return Duration::from_secs(u64::MAX);
    }

    Duration::from_secs(seconds.clamp(0.0, u64::MAX as f64) as u64)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        ADDRESS_BYTES, BYTE_VALUES, combinations, expected_attempts_for_score,
        expected_remaining_seconds_for_score, probability_for_at_least_zero_bytes,
    };

    const EPSILON: f64 = 1e-12;

    #[test]
    fn expected_attempts_for_score_zero_is_one() {
        assert_eq!(expected_attempts_for_score(0), Some(1.0));
    }

    #[test]
    fn expected_attempts_for_score_one_uses_at_least_one_zero_byte() {
        let expected_attempts = expected_attempts_for_score(1).expect("score should be possible");
        let probability = 1.0 - (255.0_f64 / 256.0).powi(ADDRESS_BYTES as i32);

        assert!((expected_attempts - 1.0 / probability).abs() < EPSILON);
    }

    #[test]
    fn probability_for_small_width_uses_binomial_at_least_score() {
        assert_eq!(probability_for_at_least_zero_bytes(2, 0), Some(1.0));
        assert_eq!(
            probability_for_at_least_zero_bytes(2, 1),
            Some(511.0 / 65_536.0)
        );
        assert_eq!(
            probability_for_at_least_zero_bytes(2, 2),
            Some(1.0 / 65_536.0)
        );
    }

    #[test]
    fn expected_attempts_for_realistic_width_known_value() {
        let expected_attempts = expected_attempts_for_score(7).expect("score should be possible");
        let probability = probability_for_at_least_zero_bytes(ADDRESS_BYTES, 7)
            .expect("score should be possible");

        assert!((expected_attempts - 1.0 / probability).abs() < 0.01);
        assert!((expected_attempts - 971_829_265_769.367_9).abs() < 0.01);
    }

    #[test]
    fn seven_zero_bytes_anywhere_is_much_easier_than_seven_leading_zero_bytes() {
        let anywhere_probability = probability_for_at_least_zero_bytes(ADDRESS_BYTES, 7)
            .expect("score should be possible");
        let leading_probability = (1.0 / BYTE_VALUES).powi(7);

        assert!(anywhere_probability > leading_probability * 70_000.0);
    }

    #[test]
    fn expected_attempts_for_score_beyond_address_bytes_is_impossible() {
        assert_eq!(expected_attempts_for_score(ADDRESS_BYTES + 1), None);
    }

    #[test]
    fn combinations_counts_address_byte_pairs() {
        assert_eq!(combinations(ADDRESS_BYTES, 2), 190.0);
    }

    #[test]
    fn doubling_hashrate_halves_expected_eta() {
        let score = 6;
        let slow = expected_remaining_seconds_for_score(score, 10_000_000.0, Duration::ZERO)
            .expect("score should be possible");
        let fast = expected_remaining_seconds_for_score(score, 20_000_000.0, Duration::ZERO)
            .expect("score should be possible");

        assert!((slow / fast - 2.0).abs() < EPSILON);
    }

    #[test]
    fn eta_is_memoryless_after_elapsed_time_without_hit() {
        let score = 6;
        let attempts_per_sec = 10_000_000.0;
        let initial = expected_remaining_seconds_for_score(score, attempts_per_sec, Duration::ZERO)
            .expect("score should be possible");
        let after_an_hour = expected_remaining_seconds_for_score(
            score,
            attempts_per_sec,
            Duration::from_secs(60 * 60),
        )
        .expect("score should be possible");

        assert_eq!(initial, after_an_hour);
    }
}
