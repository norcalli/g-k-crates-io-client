#![allow(unknown_lints)]

extern crate curl;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate url;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::prelude::*;
use std::io::Cursor;

use curl::easy::{Easy, List};
use url::percent_encoding::{percent_encode, QUERY_ENCODE_SET};

pub type Result<T> = std::result::Result<T, failure::Error>;

pub struct Registry {
    host: String,
    token: Option<String>,
    handle: Easy,
}

#[derive(PartialEq, Clone, Copy)]
pub enum Auth {
    Authorized,
    Unauthorized,
}

#[derive(Deserialize)]
pub struct Crate {
    pub name: String,
    pub description: Option<String>,
    pub max_version: String,
}

#[derive(Serialize)]
pub struct NewCrate {
    pub name: String,
    pub vers: String,
    pub deps: Vec<NewCrateDependency>,
    pub features: BTreeMap<String, Vec<String>>,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub documentation: Option<String>,
    pub homepage: Option<String>,
    pub readme: Option<String>,
    pub readme_file: Option<String>,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub license: Option<String>,
    pub license_file: Option<String>,
    pub repository: Option<String>,
    pub badges: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub links: Option<String>,
}

#[derive(Serialize)]
pub struct NewCrateDependency {
    pub optional: bool,
    pub default_features: bool,
    pub name: String,
    pub features: Vec<String>,
    pub version_req: String,
    pub target: Option<String>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}

#[derive(Deserialize)]
pub struct User {
    pub id: u32,
    pub login: String,
    pub avatar: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
}

pub struct Warnings {
    pub invalid_categories: Vec<String>,
    pub invalid_badges: Vec<String>,
}

#[derive(Deserialize)]
struct R {
    ok: bool,
}
#[derive(Deserialize)]
struct OwnerResponse {
    ok: bool,
    msg: String,
}
#[derive(Deserialize)]
struct ApiErrorList {
    errors: Vec<ApiError>,
}
#[derive(Deserialize)]
struct ApiError {
    detail: String,
}
#[derive(Serialize)]
struct OwnersReq<'a> {
    users: &'a [&'a str],
}
#[derive(Deserialize)]
struct Users {
    users: Vec<User>,
}
#[derive(Deserialize)]
struct TotalCrates {
    total: u32,
}
#[derive(Deserialize)]
struct Crates {
    crates: Vec<Crate>,
    meta: TotalCrates,
}
impl Registry {
    pub fn new(host: String, token: Option<String>) -> Registry {
        Registry::new_handle(host, token, Easy::new())
    }

    pub fn new_handle(host: String, token: Option<String>, handle: Easy) -> Registry {
        Registry {
            host,
            token,
            handle,
        }
    }

    pub fn add_owners(&mut self, krate: &str, owners: &[&str]) -> Result<String> {
        let body = serde_json::to_string(&OwnersReq { users: owners })?;
        let body = self.put(format!("/crates/{}/owners", krate), body.as_bytes())?;
        assert!(serde_json::from_str::<OwnerResponse>(&body)?.ok);
        Ok(serde_json::from_str::<OwnerResponse>(&body)?.msg)
    }

    pub fn remove_owners(&mut self, krate: &str, owners: &[&str]) -> Result<()> {
        let body = serde_json::to_string(&OwnersReq { users: owners })?;
        let body = self.delete(format!("/crates/{}/owners", krate), Some(body.as_bytes()))?;
        assert!(serde_json::from_str::<OwnerResponse>(&body)?.ok);
        Ok(())
    }

    pub fn list_owners(&mut self, krate: &str) -> Result<Vec<User>> {
        let body = self.get(format!("/crates/{}/owners", krate))?;
        Ok(serde_json::from_str::<Users>(&body)?.users)
    }

    pub fn get_crate_data(&mut self, krate: &str) -> Result<String> {
        self.handle.get(true)?;
        self.req(format!("/crates/{}", krate), None, Auth::Unauthorized)
    }

    pub fn get_crate_dependencies(&mut self, krate: &str, version: &str) -> Result<String> {
        // The following URL is supposed to be obtained from crates.io, see
        // `$krate.versions[$].links.dependencies`
        // But let's assume that it always is constant:
        // `/api/v1/crates/$krate/$version/dependencies`
        self.handle.get(true)?;
        self.req(
            format!("/crates/{}/{}/dependencies", krate, version),
            None,
            Auth::Unauthorized,
        )
    }

    pub fn publish(&mut self, krate: &NewCrate, tarball: &File) -> Result<Warnings> {
        let json = serde_json::to_string(krate)?;
        // Prepare the body. The format of the upload request is:
        //
        //      <le u32 of json>
        //      <json request> (metadata for the package)
        //      <le u32 of tarball>
        //      <source tarball>
        let stat = tarball.metadata()?;
        let header = {
            let mut w = Vec::new();
            w.extend(
                [
                    (json.len() >> 0) as u8,
                    (json.len() >> 8) as u8,
                    (json.len() >> 16) as u8,
                    (json.len() >> 24) as u8,
                ]
                .iter()
                .map(|x| *x),
            );
            w.extend(json.as_bytes().iter().map(|x| *x));
            w.extend(
                [
                    (stat.len() >> 0) as u8,
                    (stat.len() >> 8) as u8,
                    (stat.len() >> 16) as u8,
                    (stat.len() >> 24) as u8,
                ]
                .iter()
                .map(|x| *x),
            );
            w
        };
        let size = stat.len() as usize + header.len();
        let mut body = Cursor::new(header).chain(tarball);

        let url = format!("{}/api/v1/crates/new", self.host);

        let token = match self.token.as_ref() {
            Some(s) => s,
            None => bail!("no upload token found, please run `cargo login`"),
        };
        self.handle.put(true)?;
        self.handle.url(&url)?;
        self.handle.in_filesize(size as u64)?;
        let mut headers = List::new();
        headers.append("User-Agent: cargo-show")?;
        headers.append("Accept: application/json")?;
        headers.append(&format!("Authorization: {}", token))?;
        self.handle.http_headers(headers)?;

        let body = handle(&mut self.handle, &mut |buf| body.read(buf).unwrap_or(0))?;

        let response = if body.len() > 0 {
            body.parse::<serde_json::Value>()?
        } else {
            "{}".parse()?
        };

        let invalid_categories: Vec<String> = response
            .get("warnings")
            .and_then(|j| j.get("invalid_categories"))
            .and_then(|j| j.as_array())
            .map(|x| x.iter().flat_map(|j| j.as_str()).map(Into::into).collect())
            .unwrap_or_else(Vec::new);

        let invalid_badges: Vec<String> = response
            .get("warnings")
            .and_then(|j| j.get("invalid_badges"))
            .and_then(|j| j.as_array())
            .map(|x| x.iter().flat_map(|j| j.as_str()).map(Into::into).collect())
            .unwrap_or_else(Vec::new);

        Ok(Warnings {
            invalid_categories,
            invalid_badges,
        })
    }

    pub fn search(&mut self, query: &str, limit: u32) -> Result<(Vec<Crate>, u32)> {
        let formatted_query = percent_encode(query.as_bytes(), QUERY_ENCODE_SET);
        let body = self.req(
            format!("/crates?q={}&per_page={}", formatted_query, limit),
            None,
            Auth::Unauthorized,
        )?;

        let crates = serde_json::from_str::<Crates>(&body)?;
        Ok((crates.crates, crates.meta.total))
    }

    pub fn yank(&mut self, krate: &str, version: &str) -> Result<()> {
        let body = self.delete(format!("/crates/{}/{}/yank", krate, version), None)?;
        assert!(serde_json::from_str::<R>(&body)?.ok);
        Ok(())
    }

    pub fn unyank(&mut self, krate: &str, version: &str) -> Result<()> {
        let body = self.put(format!("/crates/{}/{}/unyank", krate, version), &[])?;
        assert!(serde_json::from_str::<R>(&body)?.ok);
        Ok(())
    }

    fn put(&mut self, path: String, b: &[u8]) -> Result<String> {
        self.handle.put(true)?;
        self.req(path, Some(b), Auth::Authorized)
    }

    fn get(&mut self, path: String) -> Result<String> {
        self.handle.get(true)?;
        self.req(path, None, Auth::Authorized)
    }

    fn delete(&mut self, path: String, b: Option<&[u8]>) -> Result<String> {
        self.handle.custom_request("DELETE")?;
        self.req(path, b, Auth::Authorized)
    }

    fn req(&mut self, path: String, body: Option<&[u8]>, authorized: Auth) -> Result<String> {
        self.handle.url(&format!("{}/api/v1{}", self.host, path))?;
        let mut headers = List::new();
        headers.append("User-Agent: cargo-show")?;
        headers.append("Accept: application/json")?;
        headers.append("Content-Type: application/json")?;

        if authorized == Auth::Authorized {
            let token = match self.token.as_ref() {
                Some(s) => s,
                None => bail!("no upload token found, please run `cargo login`"),
            };
            headers.append(&format!("Authorization: {}", token))?;
        }
        self.handle.http_headers(headers)?;
        match body {
            Some(mut body) => {
                self.handle.upload(true)?;
                self.handle.in_filesize(body.len() as u64)?;
                handle(&mut self.handle, &mut |buf| body.read(buf).unwrap_or(0))
            }
            None => handle(&mut self.handle, &mut |_| 0),
        }
    }
}

fn handle(handle: &mut Easy, read: &mut dyn FnMut(&mut [u8]) -> usize) -> Result<String> {
    let mut headers = Vec::new();
    let mut body = Vec::new();
    {
        let mut handle = handle.transfer();
        handle.read_function(|buf| Ok(read(buf)))?;
        handle.write_function(|data| {
            body.extend_from_slice(data);
            Ok(data.len())
        })?;
        handle.header_function(|data| {
            headers.push(String::from_utf8_lossy(data).into_owned());
            true
        })?;
        handle.perform()?;
    }

    match handle.response_code()? {
        0 => {} // file upload url sometimes
        200 => {}
        code => bail!(
            "failed to get a 200 OK response, got {}\n\
             headers:\n\
             \t{}\n\
             body:\n\
             {}",
            code,
            headers.join("\n\t"),
            String::from_utf8_lossy(&body)
        ),
    }

    let body = match String::from_utf8(body) {
        Ok(body) => body,
        Err(..) => bail!("response body was not valid utf-8"),
    };
    match serde_json::from_str::<ApiErrorList>(&body) {
        Ok(errors) => {
            let errors = errors.errors.into_iter().map(|s| s.detail);
            bail!("api errors: {}", errors.collect::<Vec<_>>().join(", "))
        }
        Err(..) => {}
    }
    Ok(body)
}
