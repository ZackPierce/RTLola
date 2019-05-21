#![deny(unsafe_code)] // disallow unsafe code by default
#![forbid(unused_must_use)] // disallow discarding errors

pub mod basics;
mod closuregen;
mod coordination;
mod evaluator;
mod storage;

use crate::coordination::Controller;
use basics::{EvalConfig, InputSource, OutputChannel, Verbosity};
use clap::{value_t, App, Arg, ArgGroup};
use std::fs::File;
use std::io::Read;
use std::time::Duration;
use streamlab_frontend;
use streamlab_frontend::ir::LolaIR;

#[derive(Debug, Clone)]
pub struct Config {
    cfg: EvalConfig,
    ir: LolaIR,
}

impl Config {
    pub fn new(args: &[String]) -> Self {
        let parse_matches = App::new("StreamLAB")
        .version(env!("CARGO_PKG_VERSION"))
        .author(env!("CARGO_PKG_AUTHORS"))
        .about("StreamLAB is a tool to analyze and monitor Lola specifications") // TODO description
        .arg(
            Arg::with_name("SPEC")
                .help("Sets the specification file to use")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("STDIN")
                .help("Read CSV input from stdin [Default]")
                .long("stdin")
        )
        .arg(
            Arg::with_name("CSV_INPUT_FILE")
                .help("Read CSV input from a file")
                .long("csv-in")
                .takes_value(true)
                .conflicts_with("STDIN")
        )
        .arg(
            Arg::with_name("CSV_TIME_COLUMN").long("csv-time-column").help("The column in the CSV that contains time info").requires("CSV_INPUT_FILE").takes_value(true)
        )
        .arg(
            Arg::with_name("STDOUT")
                .help("Output to stdout")
                .long("stdout")
        )
        .arg(
            Arg::with_name("STDERR")
                .help("Output to stderr")
                .long("stderr")
                .conflicts_with_all(&["STDOUT", "OUTPUT_FILE"])
        )
        .arg(
            Arg::with_name("DELAY")
                .short("d")
                .long("delay")
                .help("Delay [ms] between reading in two lines from the input. Only used for file input.")
                .requires("CSV_INPUT_FILE")
                .conflicts_with("OFFLINE")
                .takes_value(true)
        ).
        arg(
            Arg::with_name("VERBOSITY")
                .short("l")
                .long("verbosity")
                .possible_values(&["debug", "outputs", "triggers", "warnings", "progress", "silent", "quiet"])
                .default_value("triggers")
        )
        .arg(
            Arg::with_name("ONLINE")
                .long("online")
                .help("Use the current system time for timestamps")
        )
        .arg(
            Arg::with_name("OFFLINE")
                .long("offline")
                .help("Use the timestamps from the input.\nThe column name must be one of [time,timestamp,ts].\nThe column must produce a monotonically increasing sequence of values.")
        )
        .group(
            ArgGroup::with_name("MODE")
                .required(true)
                .args(&["ONLINE", "OFFLINE"])
        )
        .arg(
            Arg::with_name("INTERPRETED").long("interpreted").help("Interpret expressions instead of compilation")
        )
        .get_matches_from(args);

        // Now we have a reference to clone's matches
        let filename = parse_matches.value_of("SPEC").map(|s| s.to_string()).unwrap();

        let mut file = File::open(&filename).unwrap_or_else(|e| panic!("Could not open file {}: {}", filename, e));
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap_or_else(|e| panic!("Could not read file {}: {}", filename, e));

        let ir = streamlab_frontend::parse(contents.as_str());

        let delay = if parse_matches.is_present("DELAY") {
            value_t!(parse_matches, "DELAY", u32).unwrap_or_else(|_| {
                eprintln!(
                    "DELAY value `{}` is not a number.\nUsing no delay.",
                    parse_matches.value_of("DELAY").expect("We set a default value.")
                );
                0
            })
        } else {
            0
        };
        let delay = Duration::new(0, 1_000_000 * delay);

        let csv_time_column = parse_matches
            .value_of("CSV_TIME_COLUMN")
            .map(|col| col.parse::<usize>().expect("time column needs to be a unsigned integer"));

        let src = if let Some(file) = parse_matches.value_of("CSV_INPUT_FILE") {
            InputSource::with_delay(String::from(file), delay, csv_time_column)
        } else {
            InputSource::stdin()
        };

        let out = if parse_matches.is_present("STDOUT") {
            OutputChannel::StdOut
        } else if let Some(file) = parse_matches.value_of("OUTPUT_FILE") {
            OutputChannel::File(String::from(file))
        } else {
            OutputChannel::StdErr
        };

        let verbosity = match parse_matches.value_of("VERBOSITY").unwrap() {
            "debug" => Verbosity::Debug,
            "outputs" => Verbosity::Outputs,
            "triggers" => Verbosity::Triggers,
            "warnings" => Verbosity::WarningsOnly,
            "progress" => Verbosity::Progress,
            "silent" | "quiet" => Verbosity::Silent,
            _ => unreachable!(),
        };

        let closure_based_evaluator = !parse_matches.is_present("INTERPRETED");
        let offline = parse_matches.is_present("OFFLINE");

        let cfg = EvalConfig::new(src, verbosity, out, closure_based_evaluator, offline);

        Config { cfg, ir }
    }

    pub fn run(self) -> Result<Controller, Box<dyn std::error::Error>> {
        let controller = Controller::new(self.ir, self.cfg);
        controller.start()?;
        Ok(controller)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn zero_wait_time_regression() {
        let config = Config::new(&[
            "streamlab".to_string(),
            "../tests/specs/different_frequencies.lola".to_string(),
            "--csv-in=../traces/tests/alternating_single_int32-every0.1s.csv".to_string(),
            "--verbosity=silent".to_string(),
            "--offline".to_string(),
        ]);
        config.run().unwrap_or_else(|e| panic!("E2E test failed: {}", e));
    }

    #[test]
    fn test_parse_event() {
        let spec = r#"
            input bool: Bool
            input unsigned: UInt8
            input signed: Int8
            input float: Float32
            input str: String

            trigger bool = true
            trigger unsigned = 3
            trigger signed = -5
            trigger float = -123.456
            trigger str = "foobar"
        "#;
        let ir = streamlab_frontend::parse(spec);
        let mut file = NamedTempFile::new().expect("failed to create temporary file");
        write!(
            file,
            r#"float,bool,time,signed,str,unsigned
-123.456,true,1547627523.600536,-5,"foobar",3"#
        )
        .expect("writing tempfile failed");

        let cfg = EvalConfig::new(
            InputSource::for_file(file.path().to_str().unwrap().to_string(), None),
            Verbosity::Progress,
            OutputChannel::StdErr,
            true, // closure
            true, // offline
        );
        let config = Config { cfg, ir };
        let ctrl = config.run().unwrap_or_else(|e| panic!("E2E test failed: {}", e));
        macro_rules! assert_eq_num_trigger {
            ($ix:expr, $num:expr) => {
                assert_eq!(ctrl.output_handler.statistics.as_ref().unwrap().get_num_trigger($ix), $num);
            };
        }
        assert_eq_num_trigger!(0, 1);
        assert_eq_num_trigger!(1, 1);
        assert_eq_num_trigger!(2, 1);
        assert_eq_num_trigger!(3, 1);
        assert_eq_num_trigger!(4, 1);
    }

    #[test]
    fn add_two_i32_streams() {
        let spec = r#"
            input a: Int32
            input b: Int32

            output c := a + b

            trigger c > 2 "c is too large"
        "#;
        let ir = streamlab_frontend::parse(spec);
        let mut file = NamedTempFile::new().expect("failed to create temporary file");
        write!(
            file,
            "a,b,time
#,#,1547627523.000536
3,#,1547627523.100536
#,3,1547627523.200536
1,1,1547627523.300536
#,3,1547627523.400536
3,#,1547627523.500536
2,2,1547627523.600536"
        )
        .expect("writing tempfile failed");

        let cfg = EvalConfig::new(
            InputSource::for_file(file.path().to_str().unwrap().to_string(), None),
            Verbosity::Progress,
            OutputChannel::StdErr,
            true, // closure
            true, // offline
        );
        let config = Config { cfg, ir };
        let ctrl = config.run().unwrap_or_else(|e| panic!("E2E test failed: {}", e));
        assert_eq!(ctrl.output_handler.statistics.as_ref().unwrap().get_num_trigger(0), 1);
    }

    #[test]
    fn regex_simple() {
        let spec = r#"
            import regex

            input a: String

            output x := matches(a, regex: "sub")
            output y := matches(a, regex: "^sub")

            trigger x "sub"
            trigger y "^sub"
        "#;
        let ir = streamlab_frontend::parse(spec);
        let mut file = NamedTempFile::new().expect("failed to create temporary file");
        write!(
            file,
            "a,time
xub,24.8
sajhasdsub,24.9
subsub,25.0"
        )
        .expect("writing tempfile failed");

        let cfg = EvalConfig::new(
            InputSource::for_file(file.path().to_str().unwrap().to_string(), None),
            Verbosity::Progress,
            OutputChannel::StdErr,
            true, // closure
            true, // offline
        );
        let config = Config { cfg, ir };
        let ctrl = config.run().unwrap_or_else(|e| panic!("E2E test failed: {}", e));
        assert_eq!(ctrl.output_handler.statistics.as_ref().unwrap().get_num_trigger(0), 2);
        assert_eq!(ctrl.output_handler.statistics.as_ref().unwrap().get_num_trigger(1), 1);
    }
}
