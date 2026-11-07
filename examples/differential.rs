//! Hardware-free stdin/stdout fixture driver for comparison with the original TS library.
use chrono::{DateTime, Local};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    sync::Mutex,
};
use virtualdj_connect_rs::*;
struct Fixture {
    values: Mutex<HashMap<String, Value>>,
}
impl Query for Fixture {
    fn query(&self, script: &str) -> Result<String, Error> {
        let value = self
            .values
            .lock()
            .unwrap()
            .get(script)
            .cloned()
            .unwrap_or(Value::Null);
        if value.is_object() {
            Err(Error("fixture transport failure".into()))
        } else {
            Ok(value.as_str().unwrap_or("").into())
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        let value: Value = serde_json::from_str(&line?)?;
        let result = match value["op"].as_str() {
            Some("history") => {
                let now = DateTime::parse_from_rfc3339(value["now"].as_str().unwrap())?
                    .with_timezone(&Local);
                serde_json::to_value(parse_m3u(value["content"].as_str().unwrap(), now)?)?
            }
            Some("network") => {
                let mut state = NetworkState::default();
                let decks = value["decks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_u64().unwrap() as u8)
                    .collect::<Vec<_>>();
                let mut steps = Vec::new();
                for step in value["steps"].as_array().unwrap() {
                    let values = serde_json::from_value::<HashMap<String, Value>>(step.clone())?;
                    let query = Fixture {
                        values: Mutex::new(values),
                    };
                    let events = poll_network(&query, &decks, &mut state);
                    steps.push(json!({"events":events,"sandboxed":state.sandbox_active,"onAir":state.on_air(),"onAirDeck":state.on_air_deck()}));
                }
                json!(steps)
            }
            _ => return Err("unknown fixture operation".into()),
        };
        println!("{}", serde_json::to_string(&result)?);
        io::stdout().flush()?;
    }
    Ok(())
}
