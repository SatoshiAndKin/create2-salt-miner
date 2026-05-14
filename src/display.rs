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
        current_target: usize,
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
                current_target,
                eta_to_next_score(current_target, average_attempts_per_sec),
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

fn eta_to_next_score(current_target: usize, attempts_per_sec: f64) -> String {
    if attempts_per_sec <= 0.0 {
        return "calculating...".to_owned();
    }

    let Some(expected_attempts) = expected_attempts_for_score(current_target) else {
        return "not possible".to_owned();
    };

    let seconds = expected_attempts / attempts_per_sec;
    format!("{}", HumanDuration(clamped_duration_from_secs(seconds)))
}

fn expected_attempts_for_score(score: usize) -> Option<f64> {
    if score > ADDRESS_BYTES {
        return None;
    }

    let zero_probability = 1.0 / BYTE_VALUES;
    let nonzero_probability = 1.0 - zero_probability;
    let probability = (score..=ADDRESS_BYTES)
        .map(|zero_bytes| {
            combinations(ADDRESS_BYTES, zero_bytes)
                * zero_probability.powi(zero_bytes as i32)
                * nonzero_probability.powi((ADDRESS_BYTES - zero_bytes) as i32)
        })
        .sum::<f64>();

    Some(1.0 / probability)
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
    use super::{ADDRESS_BYTES, combinations, expected_attempts_for_score};

    #[test]
    fn expected_attempts_for_score_zero_is_one() {
        assert_eq!(expected_attempts_for_score(0), Some(1.0));
    }

    #[test]
    fn expected_attempts_for_score_one_uses_at_least_one_zero_byte() {
        let expected_attempts = expected_attempts_for_score(1).expect("score should be possible");
        let probability = 1.0 - (255.0_f64 / 256.0).powi(ADDRESS_BYTES as i32);

        assert!((expected_attempts - 1.0 / probability).abs() < 1e-12);
    }

    #[test]
    fn expected_attempts_for_score_beyond_address_bytes_is_impossible() {
        assert_eq!(expected_attempts_for_score(ADDRESS_BYTES + 1), None);
    }

    #[test]
    fn combinations_counts_address_byte_pairs() {
        assert_eq!(combinations(ADDRESS_BYTES, 2), 190.0);
    }
}
