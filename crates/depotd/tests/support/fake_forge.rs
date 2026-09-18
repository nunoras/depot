use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone)]
struct Route {
    method: String,
    path: String,
    query: Option<String>,
    status: u16,
    body: String,
}

pub struct FakeForge {
    port: u16,
    routes: Arc<Mutex<Vec<Route>>>,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeForge {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the fake GitHub endpoint binds");
        let port = listener
            .local_addr()
            .expect("the fake GitHub endpoint has an address")
            .port();
        let routes = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let served_routes = Arc::clone(&routes);
        let served_requests = Arc::clone(&requests);
        thread::spawn(move || serve(listener, served_routes, served_requests));
        Self {
            port,
            routes,
            requests,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn route(&self, method: &str, path: &str, status: u16, body: &str) {
        self.route_query(method, path, None, status, body);
    }

    pub fn route_query(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        status: u16,
        body: &str,
    ) {
        self.routes
            .lock()
            .expect("the routes are readable")
            .push(Route {
                method: method.to_owned(),
                path: path.to_owned(),
                query: query.map(str::to_owned),
                status,
                body: body.to_owned(),
            });
    }

    #[allow(dead_code)]
    pub fn replace_route(&self, method: &str, path: &str, status: u16, body: &str) {
        self.replace_route_query(method, path, None, status, body);
    }

    pub fn replace_route_query(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        status: u16,
        body: &str,
    ) {
        let mut routes = self.routes.lock().expect("the routes are readable");
        let route = routes.iter_mut().find(|route| {
            route.method == method && route.path == path && route.query.as_deref() == query
        });
        match route {
            Some(route) => {
                route.status = status;
                route.body = body.to_owned();
            }
            None => routes.push(Route {
                method: method.to_owned(),
                path: path.to_owned(),
                query: query.map(str::to_owned),
                status,
                body: body.to_owned(),
            }),
        }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests
            .lock()
            .expect("the requests are readable")
            .clone()
    }

    pub fn request_to(&self, path: &str) -> Recorded {
        self.requests()
            .into_iter()
            .find(|request| request.path == path)
            .unwrap_or_else(|| panic!("no request was made to {path}"))
    }
}

fn serve(
    listener: TcpListener,
    routes: Arc<Mutex<Vec<Route>>>,
    requests: Arc<Mutex<Vec<Recorded>>>,
) {
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else {
            break;
        };
        let request = read_request(&mut stream);
        let route = {
            let routes = routes.lock().expect("the routes are readable");
            routes
                .iter()
                .find(|route| {
                    route.method == request.method
                        && route.path == request.path
                        && route.query.as_ref() == request.query.as_ref()
                })
                .or_else(|| {
                    routes.iter().find(|route| {
                        route.method == request.method
                            && route.path == request.path
                            && route.query.is_none()
                    })
                })
                .cloned()
        };
        let (status, body) = match route {
            Some(route) => (route.status, route.body),
            None => (404, "{\"message\":\"Not Found\"}".to_owned()),
        };
        requests
            .lock()
            .expect("the requests are writable")
            .push(request);
        let response = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            reason(status),
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }
}

fn read_request(stream: &mut TcpStream) -> Recorded {
    let mut reader = BufReader::new(stream.try_clone().expect("the request is readable"));
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("the request line arrives");
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default();
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_owned(), Some(query.to_owned())),
        None => (target.to_owned(), None),
    };

    let mut headers = Vec::new();
    let mut length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let value = value.trim().to_owned();
            if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(0);
            }
            headers.push((name.to_ascii_lowercase(), value));
        }
    }

    let mut body = vec![0_u8; length];
    if length > 0 {
        reader.read_exact(&mut body).expect("the body arrives");
    }
    Recorded {
        method,
        path,
        query,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        _ => "Internal Server Error",
    }
}
