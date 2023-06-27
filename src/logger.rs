use std::borrow::Cow;
use console::{style, Color};
use indicatif::MultiProgress;
use log::{Record, Level, Metadata, SetLoggerError, LevelFilter};


#[derive(Default)]
struct Logger<const COLORS: bool> {
	extra_verbose: bool,
	output: Option<MultiProgress>,
}

impl<const COLORS: bool> Logger<COLORS> {
	fn new(output: Option<MultiProgress>, extra_verbose: bool) -> Self { Self { output, extra_verbose } }

	fn is_enabled(&self, metadata: &Metadata) -> bool { metadata.level() <= Level::Trace }

	fn do_flush(&self) {
		let flush = || {
			use std::io::Write;
			std::io::stdout().flush().ok();
			std::io::stderr().flush().ok();
		};
		self.output.as_ref().map(|output| output.suspend(flush));
	}
}

impl log::Log for Logger<true> {
	fn log(&self, record: &Record) {
		if !self.enabled(record.metadata()) {
			return;
		}

		let mut target: Cow<str> = record.metadata().target().into();
		let this_crate = target.starts_with(std::env!("CARGO_CRATE_NAME"));

		if !self.extra_verbose && !this_crate {
			return;
		}

		let path = {
			let line = record.line().map(|l| format!(":{l}")).unwrap_or(String::with_capacity(0));
			if this_crate {
				target = target.replacen(std::env!("CARGO_CRATE_NAME"), std::env!("CARGO_PKG_NAME"), 1)
				               .into();
			}
			style(format!("{target}{line}")).dim()
		};

		let level = match record.level() {
			Level::Warn => style(" ").bg(Color::Yellow).for_stderr(),
			Level::Error => style(" ").bg(Color::Red).for_stderr(),
			Level::Info => style(" ").bg(Color::Green).green().for_stdout(),
			Level::Debug => style(" ").bg(Color::Blue).for_stdout(),
			Level::Trace => style(" ").bg(Color::Black).dim().for_stdout(),
		};

		let render = match record.level() {
			Level::Info => format!("{level} {}", record.args()),
			Level::Warn => format!("{level} {}", record.args()),
			Level::Error => format!("{level} {} {}", path, record.args()),
			Level::Debug => format!("{level} {} {}", path, record.args()),
			Level::Trace => format!("{level} {} {}", path, record.args()),
		};

		match record.level() {
			Level::Error | Level::Warn => {
				if let Some(output) = self.output.as_ref() {
					let _ = output.println(&render).or_else(|err| {
						                               eprintln!("{err}");
						                               eprintln!("{render}");
						                               Ok::<_, !>(())
					                               });
				} else {
					eprintln!("{render}")
				}
			},
			_ => {
				// let print = || println!("{render}");
				// self.output
				//     .as_ref()
				//     .map(|output| output.suspend(print))
				//     .or_else(|| Some(print()));

				if let Some(output) = self.output.as_ref() {
					let _ = output.println(&render).or_else(|err| {
						                               eprintln!("{err}");
						                               println!("{render}");
						                               Ok::<_, !>(())
					                               });
				} else {
					println!("{render}");
				}
			},
		}
	}

	fn flush(&self) { self.do_flush() }
	fn enabled(&self, metadata: &Metadata) -> bool { self.is_enabled(metadata) }
}


impl log::Log for Logger<false> {
	fn log(&self, record: &Record) {
		if !self.enabled(record.metadata()) {
			return;
		}

		let mut target: Cow<str> = record.metadata().target().into();
		let this_crate = target.starts_with(std::env!("CARGO_CRATE_NAME"));

		if !self.extra_verbose && !this_crate {
			return;
		}

		let path = {
			let line = record.line().map(|l| format!(":{l}")).unwrap_or(String::with_capacity(0));
			if this_crate {
				target = target.replacen(std::env!("CARGO_CRATE_NAME"), std::env!("CARGO_PKG_NAME"), 1)
				               .into();
			}
			format!("{target}{line}")
		};

		let level: Cow<str> = match record.level() {
			Level::Info => "".into(),
			Level::Warn | Level::Error => record.level().as_str().into(),
			_ => record.level().as_str().chars().next().unwrap_or_default().to_string().into(),
		};

		let render = match record.level() {
			Level::Info => format!("{}", record.args()),
			Level::Warn => format!("{level} {}", record.args()),
			Level::Error => format!("{level} {} {}", path, record.args()),
			Level::Debug => format!("{level} {} {}", path, record.args()),
			Level::Trace => format!("{level} {} {}", path, record.args()),
		};

		match record.level() {
			Level::Error | Level::Warn => {
				if let Some(output) = self.output.as_ref() {
					let _ = output.println(&render).or_else(|err| {
						                               eprintln!("{err}");
						                               eprintln!("{render}");
						                               Ok::<_, !>(())
					                               });
				} else {
					eprintln!("{render}")
				}
			},
			_ => {
				if let Some(output) = self.output.as_ref() {
					let _ = output.println(&render).or_else(|err| {
						                               eprintln!("{err}");
						                               println!("{render}");
						                               Ok::<_, !>(())
					                               });
				} else {
					println!("{render}");
				}
			},
		}
	}

	fn flush(&self) { self.do_flush() }
	fn enabled(&self, metadata: &Metadata) -> bool { self.is_enabled(metadata) }
}


pub fn init(verbose: u8, output: Option<MultiProgress>) -> Result<(), SetLoggerError> {
	let max_level = match verbose {
		0 => LevelFilter::Warn,
		1 => LevelFilter::Info,
		2 => LevelFilter::Debug,
		_ => LevelFilter::Trace,
	};
	let extra_verbose = verbose > 3;

	let res = if console::colors_enabled() {
		log::set_boxed_logger(Box::new(Logger::<true>::new(output, extra_verbose)))
	} else {
		log::set_boxed_logger(Box::new(Logger::<false>::new(output, extra_verbose)))
	};
	// set level limit anyway:
	log::set_max_level(max_level);
	res
}
