use super::super::bluetooth::rfcomm_connector::ConnectionData;
use super::super::buds_config::Config;
use super::super::buds_info::{BudsInfo, BudsInfoInner};
use super::super::utils::str_to_bool;
use super::{Request, Response};

use async_std::{
    io::{prelude::*, BufReader, BufWriter},
    os::unix::net::UnixStream,
    sync::Mutex,
};
use galaxy_buds_live_rs::message::{
    bud_property::{BudProperty, EqualizerType},
    lock_touchpad, set_noise_reduction,
    simple::new_equalizer,
};
use std::sync::Arc;

/// Handle a unix socket connection
pub async fn handle_client(
    stream: UnixStream,
    cd: Arc<Mutex<ConnectionData>>,
    config: Arc<Mutex<Config>>,
) {
    let mut read_stream = BufReader::new(&stream);
    let mut write_stream = BufWriter::new(&stream);
    let mut buff = String::new();

    buff.clear();

    // Read the request
    if read_stream.read_line(&mut buff).await.is_err() {
        return;
    }

    // Parse the request
    let payload = serde_json::from_str::<Request>(buff.as_str());
    let payload = match payload {
        Ok(p) => p,
        Err(_) => return,
    };

    let get_err =
        |msg: &str| -> Response<BudsInfoInner> { Response::new_error("".to_owned(), msg, None) };

    let mut connection_data = cd.lock().await;

    // Respond with error if no device is connected
    if connection_data.get_device_count() == 0 {
        respond(&get_err("No connected device found"), &mut write_stream).await;
        return;
    }

    let device_addr = match connection_data
        .get_device_address(&payload.device.clone().unwrap_or_default().clone())
    {
        Some(addr) => addr,
        None => {
            respond(&get_err("Device not found"), &mut write_stream).await;
            return;
        }
    };

    let new_payload;

    // Run desired action
    match payload.cmd.as_str() {
        "get_status" => {
            new_payload = Response::new_success(
                device_addr.clone(),
                Some(
                    connection_data
                        .get_device(&device_addr)
                        .unwrap()
                        .inner
                        .clone(),
                ),
            );
        }
        "set_value" => {
            let mut device = connection_data.get_device_mut(&device_addr).unwrap();
            new_payload = set_buds_value(&payload, device_addr.clone(), &mut device).await
        }
        "toggle_value" => {
            let mut device = connection_data.get_device_mut(&device_addr).unwrap();
            new_payload = toggle_buds_value(&payload, device_addr.clone(), &mut device).await
        }
        "set_config" => new_payload = set_config_value(&payload, device_addr.clone(), config).await,
        _ => return,
    };

    // Respond. Return on error
    if !respond(&new_payload, &mut write_stream).await {
        return;
    }
}

// Set the value of a config option for a device
async fn set_config_value<T>(
    payload: &Request,
    address: String,
    config: Arc<Mutex<Config>>,
) -> Response<T>
where
    T: serde::ser::Serialize,
{
    let get_err = |msg: &str| -> Response<T> { Response::new_error(address.clone(), msg, None) };
    let mut config = config.lock().await;

    // Check if device already has a config entry
    // (should be available but you never know)
    if !config.has_device_config(&address) {
        return get_err("Device has no config!");
    }

    // Check required fields set
    if payload.opt_param1.is_none() || payload.opt_param2.is_none() {
        return get_err("Missing parameter");
    }

    let key = payload.opt_param1.clone().unwrap();
    let value = str_to_bool(payload.opt_param2.clone().unwrap());

    // Get the right config entry mutable
    let cfg = config.get_device_config_mut(&address);
    if cfg.is_none() {
        return get_err("error getting config!");
    }
    let mut cfg = cfg.unwrap();

    // Set the right value of the config
    match key.as_str() {
        "auto_pause" => cfg.auto_pause_music = value,
        "auto_play" => cfg.auto_resume_music = value,
        "low_battery_notification" => cfg.low_battery_notification = value,
        _ => {
            return get_err("Invalid key");
        }
    }

    // Try to save the config
    if let Err(err) = config.save().await {
        return get_err(format!("Err saving config: {}", err).as_str());
    }

    return Response::new_success(address.clone(), None);
}

async fn toggle_buds_value<T>(
    payload: &Request,
    address: String,
    device_data: &mut BudsInfo,
) -> Response<T>
where
    T: serde::ser::Serialize,
{
    let get_err = |msg: &str| -> Response<T> { Response::new_error(address.clone(), msg, None) };

    // Check required fields set
    if payload.opt_param1.is_none() {
        return get_err("Missing parameter");
    }

    let key = payload.opt_param1.clone().unwrap();
    let value = {
        match key.as_str() {
            "noise_reduction" => (!device_data.inner.noise_reduction).to_string(),
            "lock_touchpad" => (!device_data.inner.touchpads_blocked).to_string(),
            _ => {
                return get_err("Invalid key");
            }
        }
    };

    // Run desired command
    let res = set_buds_option(key.as_str(), value.as_str(), device_data).await;

    // Return success or error based on the success of the set command
    if res.is_ok() {
        Response::new_success(device_data.inner.address.clone(), None)
    } else {
        get_err(res.err().unwrap().as_str())
    }
}

async fn set_buds_value<T>(
    payload: &Request,
    address: String,
    device_data: &mut BudsInfo,
) -> Response<T>
where
    T: serde::ser::Serialize,
{
    let get_err = |msg: &str| -> Response<T> { Response::new_error(address.clone(), msg, None) };

    // Check required fields set
    if payload.opt_param1.is_none() || payload.opt_param2.is_none() {
        return get_err("Missing parameter");
    }

    let key = payload.opt_param1.clone().unwrap();
    let value = payload.opt_param2.clone().unwrap();

    // Run desired command
    let res = set_buds_option(key.as_str(), value.as_str(), device_data).await;

    // Return success or error based on the success of the set command
    if res.is_ok() {
        Response::new_success(device_data.inner.address.clone(), None)
    } else {
        get_err(res.err().unwrap().as_str())
    }
}

// Set the actual value
async fn set_buds_option(key: &str, value: &str, device_data: &mut BudsInfo) -> Result<(), String> {
    match key {
        // Set noise reduction
        "noise_reduction" => {
            let value = str_to_bool(&value);
            let msg = set_noise_reduction::new(value);
            let res = device_data.send(msg).await;
            if res.is_ok() {
                device_data.inner.noise_reduction = value;
            }
            res
        }

        // Set Touchpad lock
        "lock_touchpad" => {
            let value = str_to_bool(&value);
            let msg = lock_touchpad::new(value);
            let res = device_data.send(msg).await;
            if res.is_ok() {
                device_data.inner.touchpads_blocked = value;
            }
            res
        }

        // Set EqualizerType command
        "equalizer" => match value.parse::<u8>() {
            Ok(val) => {
                let eq_type = EqualizerType::decode(val);
                let res = device_data.send(new_equalizer(eq_type)).await;
                if res.is_ok() {
                    device_data.inner.equalizer_type = eq_type;
                }
                res
            }
            Err(_) => Err("could not parse value".to_string()),
        },
        _ => Err("Invaild key to set to".to_string()),
    }
}

// Respond to client. Return true on success
async fn respond<T>(response: &Response<T>, write_stream: &mut BufWriter<&UnixStream>) -> bool
where
    T: serde::ser::Serialize,
{
    // Write response
    if let Err(err) = write_stream
        .write(serde_json::to_string(response).unwrap().as_bytes())
        .await
    {
        eprintln!("Err: {:?}", err);
        return false;
    }

    // Flush writer
    if write_stream.flush().await.is_err() {
        return false;
    }

    true
}
