use std::error::Error;
use std::io::prelude::*;
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::daemon::bud_connection::BudsInfoInner;
use crate::daemon::unix_request_handler::{Request, Response};

pub struct SocketClient {
    #[allow(dead_code)]
    path: String,
    socket: UnixStream,
}

impl SocketClient {
    // Create a new SocketClient
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            path: path.as_ref().to_str().unwrap().to_owned(),
            socket: UnixStream::connect(path)?,
        })
    }

    /// Do a request to the daemon
    pub fn do_request(&mut self, request: Request) -> Result<String, Box<dyn Error>> {
        let mut stream = &self.socket;

        // send request
        stream.write_all(request.sendable()?.as_bytes())?;
        stream.flush()?;

        // wait for response
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    }
}

pub fn to_response<'de, T>(response_str: &'de str) -> Response<T>
where
    T: serde::ser::Serialize + serde::de::Deserialize<'de>,
{
    Response::from_string(&response_str).unwrap()
}

pub fn to_buds_info(response: String) -> Response<BudsInfoInner> {
    to_response::<BudsInfoInner>(response.as_str())
}

pub fn new_status_request(device: Option<String>) -> Request {
    Request::new("get_status".to_owned(), device)
}
