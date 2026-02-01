//! HTTP configuration server
//!
//! Serves a simple web page for configuring MQTT broker connection settings.
//! Settings are stored in NVS and persist across reboots.

use std::sync::{Arc, Mutex};

use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write;
use log::*;

use crate::config::Config;

const HTML_HEADER: &str = r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Water Controller</title>
<style>
body{font-family:sans-serif;max-width:400px;margin:40px auto;padding:0 20px}
h1{font-size:1.3em}
label{display:block;margin-top:12px;font-weight:bold}
input{width:100%;padding:6px;box-sizing:border-box;margin-top:4px}
input[type=submit]{margin-top:20px;background:#0066cc;color:#fff;border:none;
padding:10px;cursor:pointer;font-size:1em}
</style></head><body>
<h1>Water Controller Setup</h1>
"#;

const HTML_FOOTER: &str = "</body></html>";

pub struct WebServer {
    _server: EspHttpServer<'static>,
}

impl WebServer {
    pub fn start(config: Arc<Mutex<Config>>) -> anyhow::Result<Self> {
        let server_config = Configuration {
            stack_size: 10240,
            ..Default::default()
        };
        let mut server = EspHttpServer::new(&server_config)?;

        let config_get = config.clone();
        server.fn_handler::<anyhow::Error, _>("/", Method::Get, move |req| {
            let cfg = config_get.lock().unwrap();
            let body = format!(
                r#"{header}<form method="post" action="/">
<label>MQTT Broker Host</label>
<input name="broker" type="text" value="{broker}" placeholder="homeassistant.local" required>
<label>MQTT Port</label>
<input name="port" type="number" value="{port}" min="1" max="65535">
<label>Username</label>
<input name="username" type="text" value="{username}">
<label>Password</label>
<input name="password" type="password" value="{password}">
<input type="submit" value="Save &amp; Reboot">
</form>{footer}"#,
                header = HTML_HEADER,
                broker = cfg.mqtt_broker,
                port = cfg.mqtt_port,
                username = cfg.mqtt_username,
                password = cfg.mqtt_password,
                footer = HTML_FOOTER,
            );
            drop(cfg);
            let mut resp = req.into_ok_response()?;
            resp.write_all(body.as_bytes())?;
            Ok(())
        })?;

        let config_post = config.clone();
        server.fn_handler::<anyhow::Error, _>("/", Method::Post, move |mut req| {
            // Read POST body into fixed buffer
            let mut buf = [0u8; 1024];
            let mut total = 0;
            loop {
                match req.read(&mut buf[total..]) {
                    Ok(0) => break,
                    Ok(n) => total += n,
                    Err(e) => {
                        warn!("Web POST read error: {:?}", e);
                        break;
                    }
                }
                if total >= buf.len() {
                    break;
                }
            }
            let body = String::from_utf8_lossy(&buf[..total]);

            let mut broker = String::new();
            let mut port: u16 = 1883;
            let mut username = String::new();
            let mut password = String::new();

            for pair in body.split('&') {
                let mut kv = pair.splitn(2, '=');
                let key = kv.next().unwrap_or("");
                let val = url_decode(kv.next().unwrap_or(""));
                match key {
                    "broker" => broker = val,
                    "port" => port = val.parse().unwrap_or(1883),
                    "username" => username = val,
                    "password" => password = val,
                    _ => {}
                }
            }

            info!("Web config: broker={}:{} user={}", broker, port, username);

            {
                let mut cfg = config_post.lock().unwrap();
                let _ = cfg.set_mqtt_broker(&broker);
                let _ = cfg.set_mqtt_port(port);
                let _ = cfg.set_mqtt_username(&username);
                let _ = cfg.set_mqtt_password(&password);
            }

            let resp_body = format!(
                "{}<p>Settings saved. Rebooting...</p>{}",
                HTML_HEADER, HTML_FOOTER,
            );
            let mut resp = req.into_ok_response()?;
            resp.write_all(resp_body.as_bytes())?;
            drop(resp);

            // Give the response time to be sent
            std::thread::sleep(std::time::Duration::from_secs(1));
            unsafe { esp_idf_svc::sys::esp_restart(); }
        })?;

        info!("Web server started on port 80");

        Ok(Self { _server: server })
    }
}

/// Minimal URL percent-decoding and '+' to space conversion
fn url_decode(input: &str) -> String {
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(byte) = u8::from_str_radix(
                    &input[i + 1..i + 3], 16,
                ) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
