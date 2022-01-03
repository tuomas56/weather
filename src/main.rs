#![feature(let_else, backtrace)]

mod raw;

use std::{str::FromStr};
use serde::Serialize;
use clap::Parser;
use anyhow::{Context, Result, anyhow};
use comfy_table::{Table, Row, Cell};
use chrono::{NaiveDate, NaiveTime};
use raw::{Location, Forecast};
use indicatif::{ProgressBar, ProgressStyle};

#[derive(Parser, Debug, Clone)]
#[clap(about, version, author)]
struct Args {
    #[clap(
        help = "Location to forecast. Blank means current location",
        long_help = "The location you want to find a forecast for. If you leave this blank, the app will attempt to find your current location. If the location you enter is ambiguous and non-interactive mode is not enabled, you will be asked to pick a preferred location."
    )]
    location: Option<String>,

    #[clap(
        short, long, default_value = "0", 
        help = "Day to start forecasting, relative to today",
        long_help = "The number of days in the future to start the forecast from. This must be positive - zero is today, one is tomorrow, etc. This is provided on a best-effort basis, most locations have only a few days of forecasts available."
    )]
    day: usize,

    #[clap(
        short, long, default_value = "1",
        help = "Number of days to forecast",
        long_help = "The number of days past the start date to forecast. This is provided on a best-effort basis, most locations have only a few days of forecasts available. "
    )]
    count: usize,

    #[clap(
        short, long, default_value = "0:3:8", parse(try_from_str),
        help = "Time range to forecast",
        long_help = "The range of times that you want a forecast for on each day. This should be entered in the format start:step:count, where each field is an hour number 0-24 in local time. The default corresponds to the times 0:00, 4:00, 8:00, 12:00, 16:00, 20:00. This is also provided on a best effort basis - only forecasts in the future can be shown."
    )]
    time_range: TimeRange,

    #[clap(
        short, long,
        help = "Enable JSON output",
        long_help = "Enable the JSON output mode. All forecast data and errors will be output in JSON format. This does not automatically imply non-interactive mode."
    )]
    json: bool,

    #[clap(
        short, long,
        help = "Disable all interactions",
        long_help = "Enable non-interactive mode. Ambiguous locations will be rejected rather than attempting to query the user for a preferred location."
    )]
    non_interactive: bool,

    #[clap(
        short, long,
        help = "Output extra forecast data",
        long_help = "Output all available forecast data, rather than just status, temperature, perceived temperature 'feels-like', and precipitation chance. JSON output will contain all available data regardless of this flag."
    )]
    extra: bool,

    #[clap(
        short, long,
        help = "Use US customary units instead of metric",
        long_help = "Switch the unit system to use for data output to US customary units (degrees Fahrenheit and miles per hour), instead of metric units (degrees Celsius and kilometres per hour)."
    )]
    freedom_units: bool,

    #[clap(
        short, long,
        help = "Disable UTF8 and color output",
        long_help = "Disable all UTF8 and colored outputs - all outputs will use plain ASCII. Furthermore, if non-interactive mode is enabled, no escape codes will be used. The following abbreviations will be used for weather status: CL = Cloudy, SH = Showers, PC = Partly Cloudy, SU = Sunny, CN = Clear Night, SN = Snow, RA = Rain, SL = Sleet, TH = Thunderstorm."
    )]
    ascii: bool
}

#[derive(Debug, Clone)]
struct TimeRange {
    start: usize,
    step: usize,
    count: usize
}

impl FromStr for TimeRange {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let fmt_regex = regex::Regex::new("^(0?[0-9]|1[0-9]|2[0-3]):(0?[0-9]|1[0-9]|2[0-4]):(0?[0-9]|1[0-9]|2[0-4])$")?;
        let caps = fmt_regex.captures(s).context("see --help for correct format")?;
        let start = caps.get(1).context("regex error")?.as_str().parse()?;
        let step = caps.get(2).context("regex error")?.as_str().parse()?;
        let count = caps.get(3).context("regex error")?.as_str().parse()?;
        if count*(step - 1) + start >= 24 {
            Err(anyhow!("this time range overlaps the next day"))
        } else {
            Ok(TimeRange { start, step, count })
        }
    }
}

struct Mixer {
    data: Vec<(NaiveTime, Forecast)>
}

impl Mixer {
    fn new(mut data: Vec<(NaiveTime, Forecast)>) -> Mixer {
        data.sort_by_key(|(time, _)| *time);
        Mixer { data }
    }

    fn lerp(&self, time: NaiveTime) -> Option<Forecast> {
        match self.data.binary_search_by_key(&time, |(time, _)| *time) {
            Err(idx) if idx == 0 || idx == self.data.len() => None,
            Ok(idx) => Some(self.data[idx].1.clone()),
            Err(idx) => {
                let (atime, afore) = self.data[idx - 1].clone();
                let (btime, bfore) = self.data[idx].clone();
                let t = (time - atime).num_minutes() as f32 / (btime - atime).num_minutes() as f32;

                Some(Forecast {
                    status: if bfore.precipitation > afore.precipitation { bfore.status } else { afore.status },
                    precipitation: (1.0 - t)*afore.precipitation + t*bfore.precipitation,
                    temperature: (1.0 - t)*afore.temperature + t*bfore.precipitation,
                    feels_like: (1.0 - t)*afore.feels_like + t * bfore.feels_like,
                    wind_speed: (1.0 - t)*afore.wind_speed + t * bfore.wind_speed,
                    wind_direction: afore.wind_direction,
                    wind_gust: (1.0 - t)*afore.wind_gust + t * bfore.wind_gust,
                    visibility: (1.0 - t)*afore.visibility + t * bfore.visibility,
                    humidity: (1.0 - t)*afore.humidity + t * bfore.humidity,
                    uv_index: afore.uv_index.max(bfore.uv_index)
                })
            }
        }
        
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Output {
    Data { location: Location, data: Vec<DayWrapper> },
    Error { error: serde_error::Error }
}

#[derive(Debug, Serialize)]
struct DayWrapper {
    date: NaiveDate,
    times: Vec<TimeWrapper>
}

#[derive(Debug, Serialize)]
struct TimeWrapper {
    time: NaiveTime,
    forecast: Forecast
}

fn cli_main(args: Args) -> Result<(Location, Vec<DayWrapper>)> {
    let spinner_style = ProgressStyle::default_spinner()
        .tick_chars(if args.ascii { "|/-\\" } else { "🌑🌒🌓🌔🌕🌖🌗🌘" })
        .template("{prefix:.bold.dim} {spinner} {wide_msg}");

    let bar = ProgressBar::new_spinner();
    if !args.non_interactive {
        bar.set_style(spinner_style);
        bar.set_message("Finding location");
        bar.enable_steady_tick(100);
    }

    let location = if let Some(location) = raw::get_location(args.location.clone(), args.non_interactive, args.ascii, bar.clone())? {
        location
    } else {
        if !args.non_interactive {
            bar.finish_and_clear();
        }

        return Err(anyhow!("That location could not be found: perhaps there is a typo, or your location services are off."))
    };

    let geohash = if let Some(geohash) = location.geohash.clone() {
        geohash
    } else {
        if !args.non_interactive {
            bar.finish_and_clear();
        }

        return Err(anyhow!("That location is too broad, please pick a more specific location."))
    };

    if !args.non_interactive {
        bar.set_message(format!("Getting forecast for {} ({})", location.name, location.area.as_deref().unwrap_or("N/A")));
    }

    let data = raw::get_forecast(geohash, args.freedom_units)?;

    if !args.non_interactive {
        bar.finish_and_clear();
    }

    let mut odata = Vec::new();
    for (date, fs) in data.into_iter().skip(args.day).take(args.count) {
        let mixer = Mixer::new(fs);
        let mut times = Vec::new();
        let mut t = args.time_range.start;
        for _ in 0..args.time_range.count {
            let time = NaiveTime::from_hms(t as u32, 0, 0);
            t += args.time_range.step;
            let Some(forecast) = mixer.lerp(time) else { continue };
            times.push(TimeWrapper { time, forecast });
        }
        odata.push(DayWrapper { date, times });
    }

    Ok((location, odata))
}

fn format_output_failure(error: anyhow::Error) {
    println!("Error: ");
    for (i, err) in error.chain().enumerate() {
        println!("  {}: {}", i, err);
    }

    let bt = error.backtrace();
    match bt.status() {
        std::backtrace::BacktraceStatus::Captured => {
            println!("\nBacktrace:\n{}", bt);
        },
        _ => println!("\nNo backtrace captured.")
    }
}

fn format_output_success(args: Args, location: Location, data: Vec<DayWrapper>) {
    println!("Forecast for {} ({})", location.name, location.area.as_deref().unwrap_or("N/A"));

    let format_temp = |t: f32| if args.freedom_units {
        format!("{:.1}f", t)
    } else {
        format!("{:.1}C", t)
    };

    let format_speed = |t: f32| if args.freedom_units {
        format!("{:.1}mph", t)
    } else {
        format!("{:.1}kph", t)
    };

    if data.is_empty() {
        println!("No applicable data available.");
    }

    for DayWrapper { date, times: data } in data {
        let mut table = Table::new();
        let mut times = Row::new();
        let mut status = Row::new();
        let mut precip = Row::new();
        let mut temp = Row::new();
        let mut feels = Row::new();
        let mut wind = Row::new();
        let mut dir = Row::new();
        let mut gust = Row::new();
        let mut visib = Row::new();
        let mut humid = Row::new();
        let mut uv = Row::new();

        times.add_cell(Cell::new("Time"));
        status.add_cell(Cell::new("Status"));
        precip.add_cell(Cell::new("Precipitation"));
        temp.add_cell(Cell::new("Temperature"));
        feels.add_cell(Cell::new("Feels Like"));
        wind.add_cell(Cell::new("Wind Speed"));
        dir.add_cell(Cell::new("Wind Direction"));
        gust.add_cell(Cell::new("Wind Gust"));
        visib.add_cell(Cell::new("Visibility"));
        humid.add_cell(Cell::new("Humidity"));
        uv.add_cell(Cell::new("UV Index"));

        for TimeWrapper { time, forecast } in data {
            times.add_cell(Cell::new(time.format("%H:%M")));
            status.add_cell(Cell::new(match forecast.status.as_str() {
                "Cloudy" | "Overcast" => if args.ascii { "CL" } else { "☁" },
                "Light shower (night)" | "Light shower (day)" | "Heavy shower (day)" | "Heavy shower (night)" => if args.ascii { "SH" } else { "🌧" },
                "Partly cloudy (night)" | "Partly cloudy (day)" => if args.ascii { "PC" } else { "🌥" },
                "Sunny day" => if args.ascii { "SU" } else { "☀" },
                "Clear night" => if args.ascii { "CN" } else { "☾" },
                "Light snow" | "Heavy snow" => if args.ascii { "SN" } else { "☃" },
                "Sunny intervals" => if args.ascii { "PC" } else { "🌤" },
                "Heavy rain" | "Light rain" => if args.ascii { "RA" } else { "☂" },
                "Sleet" => if args.ascii { "SL" } else { "🌨" },
                "Thunder shower (night)" | "Thunder shower (day)" => if args.ascii { "TH" } else { "☈" },
                status => status
            }));
            precip.add_cell(Cell::new(format!("{}%", forecast.precipitation)));
            temp.add_cell(Cell::new(format_temp(forecast.temperature)));
            feels.add_cell(Cell::new(format_temp(forecast.feels_like)));
            wind.add_cell(Cell::new(format_speed(forecast.wind_speed)));
            dir.add_cell(Cell::new(forecast.wind_direction));
            gust.add_cell(Cell::new(format_speed(forecast.wind_gust)));
            visib.add_cell(Cell::new(forecast.visibility));
            humid.add_cell(Cell::new(format!("{}%", forecast.humidity)));
            uv.add_cell(Cell::new(forecast.uv_index));
        }

        if args.ascii {
            table.load_preset(comfy_table::presets::ASCII_BORDERS_ONLY_CONDENSED);
        } else {
            table.load_preset(comfy_table::presets::UTF8_BORDERS_ONLY)
                .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);
        }

        table.set_content_arrangement(comfy_table::ContentArrangement::Dynamic)
            .set_header(times)
            .add_row(status).add_row(precip).add_row(temp).add_row(feels);
            
        if args.extra {
            table.add_row(wind).add_row(dir).add_row(gust)
                .add_row(visib).add_row(humid).add_row(uv);
        }

        println!("{}", date.format("%e %B %Y"));
        println!("{}", table);
    }
}

fn format_json_success(location: Location, data: Vec<DayWrapper>) {
    serde_json::to_writer(std::io::stdout(), &Output::Data { location, data }).unwrap();
}

fn format_json_failure(err: anyhow::Error) {
    serde_json::to_writer(std::io::stdout(), &Output::Error { error: serde_error::Error::new(&*err) }).unwrap();
}

fn main() {
    let args = Args::parse();

    match cli_main(args.clone()) {
        Ok((location, data)) => if !args.json {
            format_output_success(args, location, data)
        } else {
            format_json_success(location, data)
        },
        Err(err) => if !args.json {
            format_output_failure(err)
        } else {
            format_json_failure(err)
        }
    }
}
