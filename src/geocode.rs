//! Best-effort address geocoding via OpenStreetMap's Nominatim. Like the
//! Instagram fetch, this is allowed to fail without consequence: an address
//! that doesn't geocode just leaves the entry off the map. No API key.
//!
//! Nominatim's usage policy requires an identifying User-Agent and asks for
//! at most ~1 request/second - fine for an admin occasionally saving an
//! address. We do not batch or background-poll it.

use std::time::Duration;

#[derive(Debug, PartialEq)]
pub struct LatLng {
    pub lat: f64,
    pub lng: f64,
}

/// Geocode a free-form address string. Returns None on any failure
/// (network, no match, rate limit, parse) - callers treat that as "no map
/// pin", never an error.
pub fn geocode(address: &str) -> Option<LatLng> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }
    let url = format!(
        "https://nominatim.openstreetmap.org/search?format=json&limit=1&q={}",
        urlencode(address)
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent("albo-directory (https://github.com/ducks/albo)")
        .build();
    let body = agent.get(&url).call().ok()?.into_string().ok()?;
    parse_first_result(&body)
}

/// Pull the first {lat, lon} pair out of a Nominatim JSON array without a
/// JSON dependency. The response is a machine-generated array of objects;
/// we scan for the first "lat":"..." and "lon":"..." string fields.
pub fn parse_first_result(json: &str) -> Option<LatLng> {
    // Empty array => no match.
    if json.trim() == "[]" {
        return None;
    }
    let lat = extract_number_field(json, "lat")?;
    let lng = extract_number_field(json, "lon")?;
    // Sanity: reject absurd coordinates rather than plot them.
    if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lng) {
        Some(LatLng { lat, lng })
    } else {
        None
    }
}

/// Find `"field":"<number>"` and parse the number. Nominatim quotes its
/// lat/lon values as strings.
fn extract_number_field(json: &str, field: &str) -> Option<f64> {
    let needle = format!("\"{field}\":\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let end = rest.find('"')?;
    rest[..end].parse::<f64>().ok()
}

/// Minimal percent-encoding for a query value (spaces and the characters
/// that would break a URL). Enough for addresses; not a general encoder.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nominatim_response() {
        let json =
            r#"[{"place_id":1,"lat":"45.5202471","lon":"-122.6741949","display_name":"Portland"}]"#;
        assert_eq!(
            parse_first_result(json),
            Some(LatLng {
                lat: 45.5202471,
                lng: -122.6741949
            })
        );
    }

    #[test]
    fn empty_and_garbage_yield_none() {
        assert_eq!(parse_first_result("[]"), None);
        assert_eq!(parse_first_result("not json"), None);
        assert_eq!(parse_first_result(""), None);
    }

    #[test]
    fn rejects_out_of_range_coords() {
        let bad = r#"[{"lat":"999.0","lon":"-122.6"}]"#;
        assert_eq!(parse_first_result(bad), None);
    }

    #[test]
    fn urlencode_handles_addresses() {
        assert_eq!(
            urlencode("123 Main St, Portland"),
            "123%20Main%20St%2C%20Portland"
        );
    }
}
