use super::{EvalConfig, Verbosity};
use csv::{Reader as CSVReader, Result as ReaderResult, StringRecord};
use std::fs::File;
use std::io::{stderr, stdin, stdout, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};
use termion::{clear, cursor};

#[derive(Debug, Clone)]
pub enum OutputChannel {
    StdOut,
    StdErr,
    File(String),
}

#[derive(Debug, Clone)]
pub enum InputSource {
    StdIn,
    File { path: String, reading_delay: Option<Duration> },
}

impl InputSource {
    pub fn for_file(path: String) -> InputSource {
        InputSource::File { path, reading_delay: None }
    }

    pub fn with_delay(path: String, delay: Duration) -> InputSource {
        InputSource::File { path, reading_delay: Some(delay) }
    }

    pub fn stdin() -> InputSource {
        InputSource::StdIn
    }
}

struct ColumnMapping {
    /// Mapping from stream index/reference to input column index
    str2col: Vec<usize>,
    /// Mapping from column index to input stream index/reference
    col2str: Vec<Option<usize>>,

    /// Column index of time (if existent)
    time_ix: Option<usize>,
}

impl ColumnMapping {
    fn from_header(names: &[&str], header: &StringRecord) -> ColumnMapping {
        let str2col: Vec<usize> = names
            .iter()
            .map(|name| {
                header
                    .iter()
                    .position(|entry| &entry == name)
                    .unwrap_or_else(|| panic!("CVS header does not contain an entry for stream {}.", name))
            })
            .collect();

        let mut col2str: Vec<Option<usize>> = vec![None; header.len()];
        for (str_ix, header_ix) in str2col.iter().enumerate() {
            col2str[*header_ix] = Some(str_ix);
        }

        let time_ix = header.iter().position(|name| name == "time" || name == "ts" || name == "timestamp");
        ColumnMapping { str2col, col2str, time_ix }
    }

    fn stream_ix_for_col_ix(&self, col_ix: usize) -> Option<usize> {
        self.col2str[col_ix]
    }

    #[allow(dead_code)]
    fn time_is_stream(&self) -> bool {
        match self.time_ix {
            None => false,
            Some(col_ix) => self.col2str[col_ix].is_some(),
        }
    }

    fn num_columns(&self) -> usize {
        self.col2str.len()
    }

    #[allow(dead_code)]
    fn num_streams(&self) -> usize {
        self.str2col.len()
    }
}

enum ReaderWrapper {
    Std(CSVReader<std::io::Stdin>),
    File(CSVReader<File>),
}

impl ReaderWrapper {
    fn read_record(&mut self, rec: &mut StringRecord) -> ReaderResult<bool> {
        match self {
            ReaderWrapper::Std(r) => r.read_record(rec),
            ReaderWrapper::File(r) => r.read_record(rec),
        }
    }

    fn get_header(&mut self) -> ReaderResult<&StringRecord> {
        match self {
            ReaderWrapper::Std(r) => r.headers(),
            ReaderWrapper::File(r) => r.headers(),
        }
    }
}

pub(crate) struct InputReader {
    reader: ReaderWrapper,
    mapping: ColumnMapping,
    record: StringRecord,
    reading_delay: Option<Duration>,
}

impl InputReader {
    pub(crate) fn from(src: InputSource, names: &[&str]) -> ReaderResult<InputReader> {
        let mut delay = None;
        let mut wrapper = match src {
            InputSource::StdIn => ReaderWrapper::Std(CSVReader::from_reader(stdin())),
            InputSource::File { path, reading_delay } => {
                delay = reading_delay;
                ReaderWrapper::File(CSVReader::from_path(path)?)
            }
        };

        let mapping = ColumnMapping::from_header(names, wrapper.get_header()?);

        Ok(InputReader { reader: wrapper, mapping, record: StringRecord::new(), reading_delay: delay })
    }

    pub(crate) fn read_blocking(&mut self) -> ReaderResult<bool> {
        if let Some(delay) = self.reading_delay {
            thread::sleep(delay);
        }

        if cfg!(debug_assertion) {
            // Reset record.
            self.record.clear();
        }

        if !self.reader.read_record(&mut self.record)? {
            return Ok(false);
        }
        assert_eq!(self.record.len(), self.mapping.num_columns());

        //TODO(marvin): this assertion seems wrong, empty strings could be valid values
        if cfg!(debug_assertion) {
            assert!(self
                .record
                .iter()
                .enumerate()
                .filter(|(ix, _)| self.mapping.stream_ix_for_col_ix(*ix).is_some())
                .all(|(_, str)| !str.is_empty()));
        }

        Ok(true)
    }

    pub(crate) fn str_ref_for_stream_ix(&self, stream_ix: usize) -> &str {
        &self.record[self.mapping.str2col[stream_ix]]
    }

    pub(crate) fn str_ref_for_time(&self) -> &str {
        assert!(self.time_index().is_some());
        &self.record[self.time_index().unwrap()]
    }

    pub(crate) fn time_index(&self) -> Option<usize> {
        self.mapping.time_ix
    }
}

pub(crate) struct OutputHandler {
    pub(crate) verbosity: Verbosity,
    channel: OutputChannel,
    file: Option<File>,
    statistics: Option<Statistics>,
}

impl OutputHandler {
    // TODO: the primary flag is just a quick hack to have only one thread drawing progress information
    // Instead, we need to make sure that there is only ever one OutputHandler
    pub(crate) fn new(config: &EvalConfig, primary: bool) -> OutputHandler {
        OutputHandler {
            verbosity: config.verbosity,
            channel: config.output_channel.clone(),
            file: None,
            statistics: if primary && config.verbosity == Verbosity::Progress { Some(Statistics::new()) } else { None },
        }
    }

    pub(crate) fn runtime_warning<F, T: Into<String>>(&self, msg: F)
    where
        F: FnOnce() -> T,
    {
        self.emit(Verbosity::WarningsOnly, msg);
    }

    #[allow(dead_code)]
    pub(crate) fn trigger<F, T: Into<String>>(&self, msg: F)
    where
        F: FnOnce() -> T,
    {
        self.emit(Verbosity::Triggers, msg);
        if let Some(statistics) = &self.statistics {
            statistics.trigger();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn debug<F, T: Into<String>>(&self, msg: F)
    where
        F: FnOnce() -> T,
    {
        self.emit(Verbosity::Debug, msg);
    }

    #[allow(dead_code)]
    pub(crate) fn output<F, T: Into<String>>(&self, msg: F)
    where
        F: FnOnce() -> T,
    {
        self.emit(Verbosity::Outputs, msg);
    }

    /// Accepts a message and forwards it to the appropriate output channel.
    /// If the configuration prohibits printing the message, `msg` is never called.
    fn emit<F, T: Into<String>>(&self, kind: Verbosity, msg: F)
    where
        F: FnOnce() -> T,
    {
        if kind <= self.verbosity {
            self.print(msg().into());
        }
    }

    fn print(&self, msg: String) {
        use crate::basics::OutputChannel;
        let _ = match self.channel {
            OutputChannel::StdOut => stdout().write((msg + "\n").as_bytes()),
            OutputChannel::StdErr => stderr().write((msg + "\n").as_bytes()),
            OutputChannel::File(_) => self.file.as_ref().unwrap().write(msg.as_bytes()),
        }; // TODO: Decide how to handle the result.
    }

    pub(crate) fn new_event(&mut self) {
        if let Some(statistics) = &mut self.statistics {
            statistics.new_event();
        }
    }

    pub(crate) fn terminate(&mut self) {
        if let Some(statistics) = &mut self.statistics {
            statistics.terminate();
        }
    }
}

struct StatisticsData {
    start: SystemTime,
    num_events: AtomicU64,
    num_triggers: AtomicU64,
}

impl StatisticsData {
    fn new() -> Self {
        Self { start: SystemTime::now(), num_events: AtomicU64::new(0), num_triggers: AtomicU64::new(0) }
    }
}

struct Statistics {
    data: Arc<StatisticsData>,
}

impl Statistics {
    fn new() -> Self {
        let data = Arc::new(StatisticsData::new());
        let copy = data.clone();
        thread::spawn(move || {
            // this thread is responsible for displaying progress information
            let mut spinner = "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈ "
                .chars()
                .cycle();
            thread::sleep(Duration::from_millis(1)); // make sure that elapsed time is greater than 0
            loop {
                Self::print_progress_info(&copy, spinner.next().unwrap());

                thread::sleep(Duration::from_millis(100));

                Self::clear_progress_info();
            }
        });

        Statistics { data }
    }

    fn new_event(&mut self) {
        self.data.num_events.fetch_add(1, Ordering::Relaxed);
    }

    fn trigger(&self) {
        self.data.num_events.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn terminate(&self) {
        Self::clear_progress_info();
        Self::print_progress_info(&self.data, ' ');
    }

    fn print_progress_info(data: &Arc<StatisticsData>, spin_char: char) {
        let mut out = std::io::stderr();

        // write statistics
        let now = SystemTime::now();
        let num_events: u128 = data.num_events.load(Ordering::Relaxed).into();
        let elapsed_total = now.duration_since(data.start).unwrap().as_nanos();
        let events_per_second = (num_events * Duration::from_secs(1).as_nanos()) / elapsed_total;
        let nanos_per_event = elapsed_total / num_events;
        writeln!(
            out,
            "{} {} events, {} events per second, {} nsec per event",
            spin_char, num_events, events_per_second, nanos_per_event
        )
        .unwrap_or_else(|_| {});
        //let num_triggers = copy.num_triggers.load(Ordering::Relaxed);
        //writeln!(out, "  {} triggers", num_triggers);
    }

    fn clear_progress_info() {
        let mut out = std::io::stderr();
        // clear screen as much as written in `print_progress_info`
        write!(out, "{}{}", cursor::Up(1), clear::CurrentLine).unwrap_or_else(|_| {});
    }
}

impl Default for OutputHandler {
    fn default() -> OutputHandler {
        OutputHandler::new(&EvalConfig::default(), false)
    }
}
