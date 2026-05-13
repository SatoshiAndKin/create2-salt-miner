use std::time::{Duration, SystemTime, UNIX_EPOCH};

use console::Term;
use indicatif::{HumanDuration, HumanFloatCount, MultiProgress, ProgressBar, ProgressStyle};

pub struct Display {
    // Extras
    start_time: u64,
    term: Term,

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
    pub fn new() -> Self {
        let mp = MultiProgress::new();
        let pb = ProgressManagers {
            time: mp.add(ProgressBar::new_spinner()),
            speed: mp.add(ProgressBar::new_spinner()),
            target: mp.add(ProgressBar::new_spinner()),
        };

        Self {
            start_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            term: Term::stdout(),
            mp,
            pb,
        }
    }

    pub fn start(&self) {
        let pb_style = ProgressStyle::with_template("{spinner:.blue} {msg}")
            .unwrap()
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

        self.term.clear_screen().unwrap();
    }

    pub fn update(&self, work_rate: u128, current_target: usize, found_salts: &Vec<String>) {
        println!("{:?}", self.start_time);

        let total_runtime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - self.start_time;

        if total_runtime != 0 {
            self.term.clear_last_lines(3 + found_salts.len()).unwrap();
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
                .set_message(format!("Current Target: {} leading bytes", current_target,));
        }

        for found_salt in found_salts {
            self.mp.println(found_salt).unwrap();
        }
    }
}

impl Default for Display {
    fn default() -> Self {
        Self::new()
    }
}
