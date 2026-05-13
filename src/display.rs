use std::time::{Duration, Instant};

use console::Term;
use eyre::{Result, WrapErr};
use indicatif::{HumanDuration, HumanFloatCount, MultiProgress, ProgressBar, ProgressStyle};

pub struct Display {
    // Extras
    start_time: Instant,
    term: Term,
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
            term: Term::stdout(),
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

        self.term
            .clear_screen()
            .wrap_err("failed to clear screen")?;
        Ok(())
    }

    pub fn update(
        &mut self,
        work_rate: u128,
        current_target: usize,
        found_salts: &[String],
    ) -> Result<()> {
        let total_runtime = self.start_time.elapsed().as_secs();

        if total_runtime != 0 {
            self.term
                .clear_last_lines(3)
                .wrap_err("failed to clear progress lines")?;
        }

        if total_runtime != 0 {
            self.pb.time.set_message(format!(
                "Total Runtime: {}",
                HumanDuration(Duration::from_secs(total_runtime)),
            ));

            self.pb.speed.set_message(format!(
                "Speed: {:.2} million attempts per second",
                HumanFloatCount(work_rate as f64 / total_runtime as f64),
            ));

            self.pb
                .target
                .set_message(format!("Current Target: {} zero bytes", current_target,));
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
