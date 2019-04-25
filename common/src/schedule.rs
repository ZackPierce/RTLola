use crate::duration::*;
use crate::math;
use lola_parser::ir::{LolaIR, StreamReference};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Deadline {
    pub pause: Duration,
    pub due: Vec<StreamReference>,
}

pub struct Schedule {
    pub gcd: Duration,
    pub hyper_period: Duration,
    pub deadlines: Vec<Deadline>,
}

impl Schedule {
    pub fn from(ir: &LolaIR) -> Schedule {
        let rates: Vec<Duration> = ir.time_driven.iter().map(|s| s.extend_rate).collect();
        let gcd = Self::find_extend_period(&rates);
        let hyper_period = Self::find_hyper_period(&rates);

        let extend_steps = Self::build_extend_steps(ir, gcd, hyper_period);
        let extend_steps = Self::apply_periodicity(&extend_steps);
        let deadlines = Self::condense_deadlines(gcd, extend_steps);

        Schedule { deadlines, gcd, hyper_period }
    }

    /// Determines the max amount of time the process can wait between successive checks for
    /// due deadlines without missing one.
    fn find_extend_period(rates: &[Duration]) -> Duration {
        assert!(!rates.is_empty());
        let rates: Vec<u128> = rates.iter().map(|r| dur_as_nanos(*r)).collect();
        let gcd = math::gcd_all(&rates);
        dur_from_nanos(gcd)
    }

    /// Determines the hyper period of the given `rates`.
    fn find_hyper_period(rates: &[Duration]) -> Duration {
        assert!(!rates.is_empty());
        let rates: Vec<u128> = rates.iter().map(|r| dur_as_nanos(*r)).collect();
        let lcm = math::lcm_all(&rates);
        dur_from_nanos(lcm)
    }

    /// Takes a vec of gdc-sized intervals. In each interval, there is the streams that need
    /// to be scheduled periodically at this point in time.
    /// Example:
    /// Hyper period: 2 seconds, gcd: 100ms, streams: (c @ .5Hz), (b @ 1Hz), (a @ 2Hz)
    /// Input:  `[[a] [b]   []  [c]]`
    /// Output: `[[a] [a,b] [a] [a,b,c]`
    fn apply_periodicity(steps: &[Vec<StreamReference>]) -> Vec<Vec<StreamReference>> {
        // Whenever there are streams in a cell at index `i`,
        // add them to every cell with index k*i within bounds, where k > 1.
        // k = 0 would always schedule them initially, so this must be skipped.
        // TODO: Skip last half of the array.
        let mut res = vec![Vec::new(); steps.len()];
        for (ix, streams) in steps.iter().enumerate() {
            if !streams.is_empty() {
                let mut k = 1;
                while let Some(target) = res.get_mut(k * (ix + 1) - 1) {
                    target.extend(streams);
                    k += 1;
                }
            }
        }
        res
    }

    /// Build extend steps for each gcd-sized time interval up to the hyper period.
    /// Example:
    /// Hyper period: 2 seconds, gcd: 100ms, streams: (c @ .5Hz), (b @ 1Hz), (a @ 2Hz)
    /// Result: `[[a] [b] [] [c]]`
    /// Meaning: `a` starts being scheduled after one gcd, `b` after two gcds, `c` after 4 gcds.
    fn build_extend_steps(ir: &LolaIR, gcd: Duration, hyper_period: Duration) -> Vec<Vec<StreamReference>> {
        let num_steps = divide_durations(hyper_period, gcd, false);
        let mut extend_steps = vec![Vec::new(); num_steps];
        for s in ir.time_driven.iter() {
            let ix = divide_durations(s.extend_rate, gcd, false) - 1;
            extend_steps[ix].push(s.reference);
        }
        extend_steps
    }

    fn condense_deadlines(gcd: Duration, extend_steps: Vec<Vec<StreamReference>>) -> Vec<Deadline> {
        let init: (u32, Vec<Deadline>) = (0, Vec::new());
        let (remaining, mut deadlines) = extend_steps.iter().fold(init, |(empty_counter, mut deadlines), step| {
            if step.is_empty() {
                (empty_counter + 1, deadlines)
            } else {
                let pause = (empty_counter + 1) * gcd;
                let deadline = Deadline { pause, due: step.clone() };
                deadlines.push(deadline);
                (0, deadlines)
            }
        });
        if remaining != 0 {
            // There is some gcd periods left at the end of the hyper period.
            // We cannot add them to the first because this would off-set the very first iteration.
            deadlines.push(Deadline { pause: remaining * gcd, due: Vec::new() });
        }
        deadlines
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    use lola_parser::ir::LolaIR;

    fn to_ir(spec: &str) -> LolaIR {
        lola_parser::parse(spec)
    }

    #[test]
    #[ignore] // TODO Max
    fn test_extension_rate_extraction() {
        let input = "input a: UInt8\n";
        let hz50 = "output b: UInt8 @50Hz := a";
        let hz40 = "output b: UInt8 @40Hz := a";
        let ms20 = "output b: UInt8 @20ms := a"; // 5Hz
        let ms1 = "output b: UInt8 @1ms := a"; // 100Hz

        let case1 = (format!("{}{}", input, hz50), 2_000);
        let case2 = (format!("{}{}{}", input, hz50, hz50), 20_000);
        let case3 = (format!("{}{}{}", input, hz50, hz40), 5_000);
        let case4 = (format!("{}{}{}", input, hz50, ms1), 1_000);
        let case5 = (format!("{}{}{}{}", input, hz50, ms20, ms1), 1_000);

        let cases = [case1, case2, case3, case4, case5];
        for (spec, expected) in cases.iter() {
            let rates: Vec<std::time::Duration> = to_ir(spec).time_driven.iter().map(|s| s.extend_rate).collect();
            let was = Schedule::find_extend_period(&rates);
            let was = crate::duration::dur_as_nanos(was);
            assert_eq!(*expected, was);
        }
    }

    #[test]
    fn test_divide_durations_round_down() {
        type TestDurations = ((u64, u32), (u64, u32), usize);
        let case1: TestDurations = ((1, 0), (1, 0), 1);
        let case2: TestDurations = ((1, 0), (0, 100_000_000), 10);
        let case3: TestDurations = ((1, 0), (0, 100_000), 10_000);
        let case4: TestDurations = ((1, 0), (0, 20_000), 50_000);
        let case5: TestDurations = ((0, 40_000), (0, 30_000), 1);
        let case6: TestDurations = ((3, 1_000), (3, 5_000), 0);

        let cases = [case1, case2, case3, case4, case5, case6];
        for (a, b, expected) in &cases {
            let to_dur = |(s, n)| Duration::new(s, n);
            let was = crate::duration::divide_durations(to_dur(*a), to_dur(*b), false);
            assert_eq!(was, *expected, "Expected {}, but was {}.", expected, was);
        }
    }

    #[test]
    fn test_divide_durations_round_up() {
        type TestDurations = ((u64, u32), (u64, u32), usize);
        let case1: TestDurations = ((1, 0), (1, 0), 1);
        let case2: TestDurations = ((1, 0), (0, 100_000_000), 10);
        let case3: TestDurations = ((1, 0), (0, 100_000), 10_000);
        let case4: TestDurations = ((1, 0), (0, 20_000), 50_000);
        let case5: TestDurations = ((0, 40_000), (0, 30_000), 2);
        let case6: TestDurations = ((3, 1_000), (3, 5_000), 1);

        let cases = [case1, case2, case3, case4, case5, case6];
        for (a, b, expected) in &cases {
            let to_dur = |(s, n)| Duration::new(s, n);
            let was = crate::duration::divide_durations(to_dur(*a), to_dur(*b), true);
            assert_eq!(was, *expected, "Expected {}, but was {}.", expected, was);
        }
    }
}