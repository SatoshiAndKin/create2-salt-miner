"""Check paired gain decisions, including common baseline drift."""

import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "metal_trials", Path(__file__).with_name("metal-trials.py")
)
assert spec is not None and spec.loader is not None
trials = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trials)


class GainGateTests(unittest.TestCase):
    def test_small_consistent_gain_passes_despite_baseline_drift(self):
        pairs = [
            {
                "baseline": {"metrics": {"attempts_per_sec": baseline}},
                "candidate": {"metrics": {"attempts_per_sec": baseline * 1.01}},
            }
            for baseline in (100, 80, 130, 110, 90)
        ]
        gains = trials.paired_gains(pairs)
        self.assertTrue(trials.gain_gate([gains, gains])["passes_gain_gate"])

    def test_mixed_sign_noise_does_not_pass(self):
        result = trials.gain_gate(
            [[-0.04, 0.05, -0.02, 0.01, 0.03], [0.02, -0.04, 0.01, -0.03, 0.02]]
        )
        self.assertFalse(result["passes_gain_gate"])
        self.assertLess(result["paired_bootstrap_95_interval"][0], 0)

    def test_independent_confirmation_is_required(self):
        result = trials.gain_gate([[0.05] * 5, [-0.01] * 5])
        self.assertFalse(result["passes_gain_gate"])
        self.assertFalse(trials.gain_gate([[0.0] * 5, [0.0] * 5])["passes_gain_gate"])
        with self.assertRaises(ValueError):
            trials.gain_gate([[0.05] * 10])


if __name__ == "__main__":
    unittest.main()
