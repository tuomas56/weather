use std::process::Command;
use anyhow::{Context, Result, anyhow};
use serde::{Serialize, Deserialize};
use dialoguer::{Select, theme};
use console::Term;
use chrono::{NaiveDate, NaiveTime};

fn get_current_location() -> Result<(f32, f32)> {
    let command = Command::new("powershell")
        .args(&["-encodedCommand", "QQBkAGQALQBUAHkAcABlACAALQBBAHMAcwBlAG0AYgBsAHkATgBhAG0AZQAgAFMAeQBzAHQAZQBtAC4ARABlAHYAaQBjAGUACgAkAEcAZQBvAFcAYQB0AGMAaABlAHIAIAA9ACAATgBlAHcALQBPAGIAagBlAGMAdAAgAFMAeQBzAHQAZQBtAC4ARABlAHYAaQBjAGUALgBMAG8AYwBhAHQAaQBvAG4ALgBHAGUAbwBDAG8AbwByAGQAaQBuAGEAdABlAFcAYQB0AGMAaABlAHIACgAkAEcAZQBvAFcAYQB0AGMAaABlAHIALgBTAHQAYQByAHQAKAApAAoACgB3AGgAaQBsAGUAIAAoACgAJABHAGUAbwBXAGEAdABjAGgAZQByAC4AUwB0AGEAdAB1AHMAIAAtAG4AZQAgACcAUgBlAGEAZAB5ACcAKQAgAC0AYQBuAGQAIAAoACQARwBlAG8AVwBhAHQAYwBoAGUAcgAuAFAAZQByAG0AaQBzAHMAaQBvAG4AIAAtAG4AZQAgACcARABlAG4AaQBlAGQAJwApACkAIAB7AAoAIAAgACAAIABTAHQAYQByAHQALQBTAGwAZQBlAHAAIAAtAE0AaQBsAGwAaQBzAGUAYwBvAG4AZABzACAAMQAwADAACgB9ACAAIAAKAAoAaQBmACAAKAAkAEcAZQBvAFcAYQB0AGMAaABlAHIALgBQAGUAcgBtAGkAcwBzAGkAbwBuACAALQBlAHEAIAAnAEQAZQBuAGkAZQBkACcAKQB7AAoAIAAgACAAIABXAHIAaQB0AGUALQBPAHUAdABwAHUAdAAgACcATgBPACcACgB9ACAAZQBsAHMAZQAgAHsACgAgACAAIAAgAFcAcgBpAHQAZQAtAE8AdQB0AHAAdQB0ACAAJwBPAEsAJwA7ACAAVwByAGkAdABlAC0ATwB1AHQAcAB1AHQAIAAkAEcAZQBvAFcAYQB0AGMAaABlAHIALgBQAG8AcwBpAHQAaQBvAG4ALgBMAG8AYwBhAHQAaQBvAG4ALgBMAGEAdABpAHQAdQBkAGUAOwAgAFcAcgBpAHQAZQAtAE8AdQB0AHAAdQB0ACAAJABHAGUAbwBXAGEAdABjAGgAZQByAC4AUABvAHMAaQB0AGkAbwBuAC4ATABvAGMAYQB0AGkAbwBuAC4ATABvAG4AZwBpAHQAdQBkAGUACgB9AA=="])
        .output()?;
    let output = String::from_utf8(command.stdout)?;

    match &output[..2] {
        "OK" => {
            let mut iter = output.lines().skip(1).map(str::parse::<f32>);
            let latitude = iter.next().context("malformed powershell output")??;
            let longitude = iter.next().context("malformed powershell output")??;
            Ok((latitude, longitude))
        },
        "NO" => Err(anyhow!("permission denied or location unavailable")),
        _ => Err(anyhow!("malformed powershell output"))
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Location {
    pub name: String,
    pub area: Option<String>,
    pub geohash: Option<String>
}

enum LocationFilter {
    Domestic,
    Beaches,
    NoCountries,
    NoUKRegions
}

fn raw_search_location(term: &str, filters: &[LocationFilter]) -> Result<Vec<Location>> {
    let filter = filters.iter().map(|filter| match filter {
        LocationFilter::Domestic => "domestic",
        LocationFilter::Beaches => "beaches",
        LocationFilter::NoCountries => "no-countries",
        LocationFilter::NoUKRegions => "no-uk-regions"
    }).collect::<String>();
    let term = urlencoding::encode(term);
    let url = format!("https://www.metoffice.gov.uk/plain-rest-services/location-search/{}/?filter={}", term, filter);

    Ok(reqwest::blocking::get(url)?.json::<Vec<Location>>()?)
}

#[derive(Debug, Clone)]
enum FoundLocation {
    Found(Location),
    Ambiguous(Vec<Location>),
    NotFound
}

fn search_location(term: &str, filters: &[LocationFilter]) -> Result<FoundLocation> {
    let cleaning_regex = regex::Regex::new(r"\s+")?;
    let term = cleaning_regex.replace_all(term.trim(), " ").to_ascii_lowercase();
    
    let postcode_regex = regex::Regex::new("^([a-zA-Z]{1,2}[0-9][a-zA-Z0-9]?) ?([0-9][a-zA-Z]{0,2})?$")?;
    let cleaned = if let Some(captures) = postcode_regex.captures(&term) {
        captures.get(1).context("malformed regex result")?.as_str().to_ascii_uppercase()
    } else {
        term
    };

    let results = raw_search_location(&cleaned, filters)?;
    
    if results.len() == 0 {
        Ok(FoundLocation::NotFound)
    } else if results.len() == 1 {
        Ok(FoundLocation::Found(results[0].clone()))
    } else {
        let mut same = results.iter().filter(|loc| {
            let name = loc.name.trim().to_ascii_lowercase();
            name == cleaned
        });

        let loc = same.next();
        let amb = same.next().is_some();

        if let Some(loc) = loc {
            if amb {
                Ok(FoundLocation::Ambiguous(results))
            } else {
                Ok(FoundLocation::Found(loc.clone()))
            }
        } else {
            Ok(FoundLocation::Ambiguous(results))
        }
    }
}

#[derive(Deserialize, Debug)]
struct NearestLocations {
    #[serde(rename = "locationResults")]
    locations: Vec<NearestLocationEntry>
}

#[derive(Deserialize, Debug)]
struct NearestLocationEntry {
    result: Location,
    distance: f32
}

fn nearest_location(latitude: f32, longitude: f32) -> Result<FoundLocation> {
    let url = format!("https://www.metoffice.gov.uk/plain-rest-services/nearest-locations?latitude={}&longitude={}", latitude, longitude);
    let results = reqwest::blocking::get(url)?.json::<NearestLocations>()?.locations;
    if results.len() == 0 {
        Ok(FoundLocation::NotFound)
    } else if results.len() == 1 {
        Ok(FoundLocation::Found(results[0].result.clone()))
    } else {
        Ok(results.into_iter()
            .min_by_key(|entry| ordered_float::OrderedFloat(entry.distance))
            .map(|entry| FoundLocation::Found(entry.result))
            .unwrap_or(FoundLocation::NotFound))
    }
}

pub fn get_location(location: Option<String>, non_interactive: bool, ascii: bool, bar: indicatif::ProgressBar) -> Result<Option<Location>> {
    let possibles = match location {
        None => {
            let (latitude, longitude) = get_current_location()?;
            nearest_location(latitude, longitude)?
        },
        Some(term) => search_location(&term, &[])?
    };

    match possibles {
        FoundLocation::NotFound => Ok(None),
        FoundLocation::Found(loc) => Ok(Some(loc)),
        FoundLocation::Ambiguous(locs) => {
            if non_interactive {
                return Ok(None)
            }

            bar.finish_and_clear();

            let items: Vec<String> = locs.iter().map(|l| {
                format!("{} ({})", l.name, l.area.as_deref().unwrap_or("N/A"))
            }).collect();
            let theme: Box<dyn theme::Theme> = if ascii {
                Box::new(theme::SimpleTheme)
            } else {
                Box::new(theme::ColorfulTheme::default())
            };
            let selection = Select::with_theme(&*theme)
                .with_prompt("That location is ambiguous - please pick one of the following")
                .items(&items)
                .default(0)
                .clear(true)
                .interact_on(&Term::stderr())?;

            bar.reset();
            bar.enable_steady_tick(100);

            Ok(Some(locs[selection].clone()))
        }
    }
} 

#[derive(Debug, Default, Clone, Serialize)]
pub struct Forecast {
    pub status: String,
    pub precipitation: f32,
    pub temperature: f32,
    pub feels_like: f32,
    pub wind_speed: f32,
    pub wind_direction: String,
    pub wind_gust: f32,
    pub visibility: f32,
    pub humidity: f32,
    pub uv_index: f32
}

pub fn get_forecast(geohash: String, freedom_units: bool) -> Result<Vec<(NaiveDate, Vec<(NaiveTime, Forecast)>)>> {
    let convert_temp = |t: f32| if freedom_units {
        t * 1.8 + 32.0
    } else {
        t
    };

    let convert_speed = |s: f32| if freedom_units {
        s * 2.237
    } else {
        s * 3.6
    };

    let day_selector = scraper::Selector::parse(".forecast-day").ok().context("can't parse selector")?;
    let time_selector = scraper::Selector::parse(".step-time > th[scope=\"col\"]").ok().context("can't parse selector")?;
    let status_selector = scraper::Selector::parse(".step-symbol > td > img").ok().context("can't parse selector")?;
    let precip_selector = scraper::Selector::parse(".step-pop > td").ok().context("can't parse selector")?;
    let temp_selector = scraper::Selector::parse(".step-temp > td > div").ok().context("can't parse selector")?;
    let feels_selector = scraper::Selector::parse(".step-feels-like > td").ok().context("can't parse selector")?;
    let wind_speed_selector = scraper::Selector::parse(".step-wind > td > div > .speed").ok().context("can't parse selector")?;
    let wind_dir_selector = scraper::Selector::parse(".step-wind > td > div > .direction").ok().context("can't parse selector")?;
    let wind_gust_selector = scraper::Selector::parse(".step-wind-gust > td > .gust").ok().context("can't parse selector")?;
    let visib_selector = scraper::Selector::parse(".step-visibility > td > .visibility").ok().context("can't parse selector")?;
    let humid_selector = scraper::Selector::parse(".step-humidity > td").ok().context("can't parse selector")?;
    let uv_selector = scraper::Selector::parse(".step-uv > td").ok().context("can't parse selector")?;

    let url = format!("https://www.metoffice.gov.uk/weather/forecast/{}", geohash);
    let html = reqwest::blocking::get(url)?.text()?;
    let doc = scraper::Html::parse_document(&html);

    let mut results = Vec::new();
    for day in doc.select(&day_selector) {
        let id = day.value().id().context("can't find id of forecast-day")?;
        let date = chrono::NaiveDate::parse_from_str(id, "%Y-%m-%d")?;

        let mut times = Vec::new();
        for time in day.select(&time_selector) {
            let data_time = time.value().attr("data-time").context("can't find data-time in step-time")?;
            times.push(chrono::NaiveTime::parse_from_str(data_time, "%H:%M")?);
        }

        let mut forecasts = vec![Forecast::default(); times.len()];

        for (i, status) in day.select(&status_selector).enumerate() {
            let title = status.value().attr("title").context("can't find title in step-symbol")?;
            forecasts[i].status = title.to_string();
        }

        for (i, precip) in day.select(&precip_selector).enumerate() {
            let inner = precip.inner_html();
            let text = inner.trim().strip_suffix('%').unwrap_or("0.0");
            let precip = match text {
                "&lt;5" => 0.0,
                _ => text.parse::<f32>()?
            };
            forecasts[i].precipitation = precip;
        }

        for (i, temp) in day.select(&temp_selector).enumerate() {
            let data_value = temp.value().attr("data-value").context("can't find data-value in step-temp")?;
            forecasts[i].temperature = convert_temp(data_value.parse()?);
        }

        for (i, feels) in day.select(&feels_selector).enumerate() {
            let data_value = feels.value().attr("data-value").context("can't find data-value in step-feels-like")?;
            forecasts[i].feels_like = convert_temp(data_value.parse()?);
        }
        
        for (i, speed) in day.select(&wind_speed_selector).enumerate() {
            let data_value = speed.value().attr("data-value").context("can't find data-value in step-wind-speed")?;
            forecasts[i].wind_speed = convert_speed(data_value.parse()?);
        }

        for (i, dir) in day.select(&wind_dir_selector).enumerate() {
            let data_value = dir.value().attr("data-value").context("can't find data-value in step-wind-direction")?;
            forecasts[i].wind_direction = data_value.to_string();
        }

        for (i, gust) in day.select(&wind_gust_selector).enumerate() {
            let data_value = gust.value().attr("data-value").context("can't find data-value in step-wind-gust")?;
            forecasts[i].wind_gust = convert_speed(data_value.parse()?);
        }

        for (i, visib) in day.select(&visib_selector).enumerate() {
            let data_value = visib.value().attr("data-value").context("can't find data-value in step-visibility")?;
            forecasts[i].visibility = data_value.parse()?;
        }

        for (i, humid) in day.select(&humid_selector).enumerate() {
            let inner = humid.inner_html();
            let text = inner.trim().strip_suffix('%').unwrap_or("0.0");
            forecasts[i].humidity = text.parse::<f32>()?;
        }

        for (i, uv) in day.select(&uv_selector).enumerate() {
            let data_value = uv.value().attr("data-value").context("can't find data-value in step-uv")?;
            forecasts[i].uv_index = data_value.parse()?;
        }

        results.push((date, times.into_iter().zip(forecasts).collect()));
    }

    Ok(results)
}