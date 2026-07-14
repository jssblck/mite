//! Local developer UI for authoring and reviewing eval labels.
//!
//! This intentionally lives outside the main `mite` CLI surface. The binary
//! serves a small browser app, keeps every file operation confined to `eval/`,
//! and reuses the production OCR, dictionary, and scoring code paths.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::artifact;
use crate::config::{AppConfig, RuntimeBackend};
use crate::dictionary::Dictionary;
use crate::eval::{
    self, BoundsTolerance, EvalReport, EvalSpec, ExpectedDetection, OcrLookupResult,
};
use crate::geometry::Rect;
use crate::ocr::{OcrEngine, build_ocr_engine};
use crate::text_geometry::measure_text_geometry;

const INDEX_HTML: &str = include_str!("static/index.html");
const APP_CSS: &str = include_str!("static/app.css");
const APP_JS: &str = include_str!("static/app.js");
const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct EvalUiOptions {
    pub config_path: PathBuf,
    pub eval_root: PathBuf,
    pub lexicon: PathBuf,
    pub host: String,
    pub port: u16,
    pub fixture_ocr: bool,
    pub min_iou: f32,
}

pub fn run_eval_ui(options: EvalUiOptions) -> Result<()> {
    if !(0.0..=1.0).contains(&options.min_iou) {
        bail!("--min-iou must be in [0, 1], got {}", options.min_iou);
    }
    let state = Arc::new(ServerState::new(options)?);
    let bind_addr = format!("{}:{}", state.host, state.port);
    let listener = TcpListener::bind(&bind_addr)
        .with_context(|| format!("failed to bind eval UI server at {bind_addr}"))?;
    let addr = listener.local_addr()?;
    println!("mite eval UI listening at http://{addr}/");
    println!("eval root: {}", state.eval_root.display());
    println!("press Ctrl+C to stop");

    for stream in listener.incoming() {
        let stream = stream.context("failed to accept eval UI connection")?;
        let state = Arc::clone(&state);
        thread::spawn(move || {
            if let Err(error) = handle_connection(stream, &state) {
                tracing::warn!("eval UI request failed: {error:#}");
            }
        });
    }
    Ok(())
}

struct ServerState {
    config: AppConfig,
    eval_root: PathBuf,
    lexicon: PathBuf,
    host: String,
    port: u16,
    min_iou: f32,
    dict: Mutex<Option<Dictionary>>,
    engine: Mutex<Option<Box<dyn OcrEngine + Send>>>,
}

impl ServerState {
    fn new(options: EvalUiOptions) -> Result<Self> {
        let mut config = if options.config_path.exists() {
            AppConfig::load(&options.config_path)?
        } else {
            AppConfig::default()
        };
        if options.fixture_ocr {
            config.runtime.backend = RuntimeBackend::Fixture;
        }
        let eval_root = options.eval_root;
        fs::create_dir_all(&eval_root)
            .with_context(|| format!("failed to create eval root {}", eval_root.display()))?;
        let eval_root = eval_root
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", eval_root.display()))?;
        Ok(Self {
            config,
            eval_root,
            lexicon: options.lexicon,
            host: options.host,
            port: options.port,
            min_iou: options.min_iou,
            dict: Mutex::new(None),
            engine: Mutex::new(None),
        })
    }

    fn with_dictionary<T>(&self, f: impl FnOnce(&Dictionary) -> Result<T>) -> Result<T> {
        let mut guard = self
            .dict
            .lock()
            .map_err(|_| anyhow::anyhow!("dictionary lock was poisoned"))?;
        if guard.is_none() {
            *guard = Some(Dictionary::load(&self.lexicon)?);
        }
        let dictionary = guard.as_ref().context("dictionary was not initialized")?;
        f(dictionary)
    }

    fn with_engine<T>(&self, f: impl FnOnce(&mut dyn OcrEngine) -> Result<T>) -> Result<T> {
        let mut guard = self
            .engine
            .lock()
            .map_err(|_| anyhow::anyhow!("OCR engine lock was poisoned"))?;
        if guard.is_none() {
            *guard = Some(build_ocr_engine(&self.config.runtime, &self.config.models)?);
        }
        let engine = guard.as_mut().context("OCR engine was not initialized")?;
        f(&mut **engine)
    }

    fn resolve_existing(&self, rel: &str) -> Result<PathBuf> {
        let rel = clean_relative_path(rel)?;
        let path = self.eval_root.join(rel);
        let canonical_root = self
            .eval_root
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", self.eval_root.display()))?;
        let canonical_path = path
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", path.display()))?;
        if !canonical_path.starts_with(canonical_root) {
            bail!("path escapes eval root: {}", path.display());
        }
        Ok(canonical_path)
    }

    fn resolve_for_write(&self, rel: &str) -> Result<PathBuf> {
        let rel = clean_relative_path(rel)?;
        Ok(self.eval_root.join(rel))
    }

    fn resolve_bundle_image(&self, bundle_rel: &str) -> Result<PathBuf> {
        let bundle_path = self.resolve_existing(bundle_rel)?;
        ensure_eval_bundle(&bundle_path)?;
        find_bundle_image(&bundle_path).with_context(|| {
            format!(
                "{} does not contain underlying.png/jpg",
                bundle_path.display()
            )
        })
    }

    fn list_bundles(&self) -> Result<BundleIndexResponse> {
        let mut bundles = Vec::new();
        scan_eval_bundles(&self.eval_root, Path::new(""), &mut bundles)?;
        bundles.sort_by(|a, b| a.bundle_path.cmp(&b.bundle_path));
        let labeled_count = bundles.iter().filter(|bundle| bundle.labeled).count();
        let unlabeled_count = bundles.len() - labeled_count;
        Ok(BundleIndexResponse {
            root: self.eval_root.display().to_string(),
            bundle_count: bundles.len(),
            labeled_count,
            unlabeled_count,
            bundles,
        })
    }

    fn load_label(&self, bundle_rel: &str) -> Result<LabelResponse> {
        let image_path = self.resolve_bundle_image(bundle_rel)?;
        let label_path = label_path_for_image(&image_path)?;
        let spec = if label_path.exists() {
            let raw = fs::read_to_string(&label_path)
                .with_context(|| format!("failed to read {}", label_path.display()))?;
            serde_json::from_str::<EvalSpec>(&raw)
                .with_context(|| format!("failed to parse {}", label_path.display()))?
        } else {
            default_eval_spec(&image_path)?
        };
        let validation_error = eval::parse_eval_spec(spec.clone())
            .err()
            .map(|error| error.to_string());
        let raw = serde_json::to_string_pretty(&spec)?;
        Ok(LabelResponse {
            path: rel_string(&self.eval_root, &label_path).ok(),
            exists: label_path.exists(),
            raw,
            spec,
            validation_error,
        })
    }

    fn save_label(&self, bundle_rel: &str, mut spec: EvalSpec) -> Result<LabelResponse> {
        let image_path = self.resolve_bundle_image(bundle_rel)?;
        let label_path = label_path_for_image(&image_path)?;
        if spec.image.is_none() {
            spec.image = image_path
                .file_name()
                .map(|name| name.to_string_lossy().to_string());
        }
        if spec.source_capture.is_none() {
            let capture = image_path.with_file_name("capture.json");
            if capture.exists() {
                spec.source_capture = Some("capture.json".to_string());
            }
        }
        let spec = eval::parse_eval_spec(spec)?;
        artifact::write_json_pretty(&label_path, spec.get())?;
        self.load_label(bundle_rel)
    }

    fn synthesize_detection(
        &self,
        bundle_rel: &str,
        draft: DraftDetectionRequest,
    ) -> Result<ExpectedDetection> {
        let image_path = self.resolve_bundle_image(bundle_rel)?;
        let image = image::open(&image_path)
            .with_context(|| format!("failed to open {}", image_path.display()))?
            .to_rgb8();
        let characters = draft
            .text
            .chars()
            .map(|ch| ch.to_string())
            .collect::<Vec<_>>();
        let character_refs = characters.iter().map(String::as_str).collect::<Vec<_>>();
        let geometry = measure_text_geometry(&image, draft.bounds, &character_refs)
            .context("could not find reliable character geometry inside the drawn text region")?;
        self.with_dictionary(|dict| {
            eval::draft_expected_detection(
                dict,
                draft.id,
                draft.text,
                geometry.line_bounds,
                geometry.character_bounds,
                draft.bounds_tolerance,
                draft.notes,
            )
        })
    }

    fn run_detections(&self, bundle_rel: &str) -> Result<DetectionRunResponse> {
        let image_path = self.resolve_bundle_image(bundle_rel)?;
        let result = self.with_dictionary(|dict| {
            self.with_engine(|engine| {
                eval::ocr_lookup_image(engine, &self.config.pipeline, dict, &image_path)
            })
        })?;
        let label_path = label_path_for_image(&image_path)?;
        let report = if label_path.exists() {
            let spec = eval::load_eval_spec(&label_path)?;
            Some(eval::score_ocr_lookup(
                &image_path,
                &label_path,
                &spec,
                &result,
                self.min_iou,
            ))
        } else {
            None
        };
        Ok(DetectionRunResponse { result, report })
    }

    fn upload_bundle(
        &self,
        dir_rel: &str,
        original_name: &str,
        body: &[u8],
    ) -> Result<BundleEntry> {
        let dir = self.resolve_for_write(dir_rel)?;
        let ext = upload_extension(original_name, body)?;
        let folder = dir.join(format!("capture-{}", artifact::unix_ms()));
        fs::create_dir_all(&folder)
            .with_context(|| format!("failed to create {}", folder.display()))?;
        let image_path = folder.join(format!("underlying.{ext}"));
        fs::write(&image_path, body)
            .with_context(|| format!("failed to write {}", image_path.display()))?;
        summarize_bundle(&self.eval_root, &folder, &image_path)
    }
}

fn handle_connection(mut stream: TcpStream, state: &ServerState) -> Result<()> {
    let response = match read_request(&mut stream).and_then(|request| route_request(request, state))
    {
        Ok(response) => response,
        Err(error) => json_error(500, &error.to_string()),
    };
    stream.write_all(&response.to_bytes())?;
    stream.flush()?;
    Ok(())
}

fn route_request(request: HttpRequest, state: &ServerState) -> Result<HttpResponse> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => Ok(text_response(200, "text/html; charset=utf-8", INDEX_HTML)),
        ("GET", "/app.css") => Ok(text_response(200, "text/css; charset=utf-8", APP_CSS)),
        ("GET", "/app.js") => Ok(text_response(
            200,
            "application/javascript; charset=utf-8",
            APP_JS,
        )),
        ("GET", "/api/bundles") => json_response(200, &state.list_bundles()?),
        ("GET", "/api/bundle-image") => {
            let bundle = required_query(&request, "bundle")?;
            let path = state.resolve_bundle_image(bundle)?;
            let bytes =
                fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
            Ok(bytes_response(200, image_content_type(&path), bytes))
        }
        ("GET", "/api/label") => {
            let bundle = required_query(&request, "bundle")?;
            json_response(200, &state.load_label(bundle)?)
        }
        ("PUT", "/api/label") => {
            let bundle = required_query(&request, "bundle")?;
            let spec = serde_json::from_slice::<EvalSpec>(&request.body)
                .context("request body must be an eval label JSON object")?;
            json_response(200, &state.save_label(bundle, spec)?)
        }
        ("POST", "/api/synthesize") => {
            let bundle = required_query(&request, "bundle")?;
            let draft = serde_json::from_slice::<DraftDetectionRequest>(&request.body)
                .context("request body must be a draft detection JSON object")?;
            json_response(
                200,
                &DraftDetectionResponse {
                    detection: state.synthesize_detection(bundle, draft)?,
                },
            )
        }
        ("POST", "/api/detections") => {
            let bundle = required_query(&request, "bundle")?;
            json_response(200, &state.run_detections(bundle)?)
        }
        ("POST", "/api/upload") => {
            let dir = request.query.get("dir").map(String::as_str).unwrap_or("");
            let name = required_query(&request, "name")?;
            json_response(200, &state.upload_bundle(dir, name, &request.body)?)
        }
        _ => Ok(json_error(404, "not found")),
    }
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    query: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut data = Vec::new();
    let header_end = loop {
        let mut chunk = [0u8; 8192];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            bail!("connection closed before request headers");
        }
        data.extend_from_slice(&chunk[..read]);
        if data.len() > MAX_HEADER_BYTES {
            bail!("request headers exceed {MAX_HEADER_BYTES} bytes");
        }
        if let Some(index) = find_header_end(&data) {
            break index;
        }
    };
    let header_bytes = &data[..header_end];
    let header_text = std::str::from_utf8(header_bytes).context("request headers are not UTF-8")?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().context("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("missing method")?.to_string();
    let target = parts.next().context("missing target")?;
    let (path, query) = split_target(target)?;
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .context("invalid content-length header")?
        .unwrap_or(0);
    let body_start = header_end + 4;
    let mut body = data[body_start..].to_vec();
    while body.len() < content_length {
        let mut chunk = vec![0u8; content_length - body.len()];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            bail!("connection closed before request body completed");
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(HttpRequest {
        method,
        path,
        query,
        body,
    })
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|window| window == b"\r\n\r\n")
}

fn split_target(target: &str) -> Result<(String, HashMap<String, String>)> {
    let (path, query_text) = target.split_once('?').unwrap_or((target, ""));
    let path = percent_decode(path)?;
    let mut query = HashMap::new();
    for pair in query_text.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        query.insert(percent_decode(name)?, percent_decode(value)?);
    }
    Ok((path, query))
}

fn required_query<'a>(request: &'a HttpRequest, name: &str) -> Result<&'a str> {
    request
        .query
        .get(name)
        .map(String::as_str)
        .with_context(|| format!("missing required query parameter {name}"))
}

fn percent_decode(input: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(input.len());
    let input = input.as_bytes();
    let mut index = 0;
    while index < input.len() {
        match input[index] {
            b'%' if index + 2 < input.len() => {
                let hex = std::str::from_utf8(&input[index + 1..index + 3])?;
                let value = u8::from_str_radix(hex, 16)
                    .with_context(|| format!("invalid percent escape %{hex}"))?;
                bytes.push(value);
                index += 3;
            }
            b'+' => {
                bytes.push(b' ');
                index += 1;
            }
            byte => {
                bytes.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(bytes).context("percent-decoded value is not UTF-8")
}

struct HttpResponse {
    status: u16,
    reason: &'static str,
    content_type: String,
    body: Vec<u8>,
}

impl HttpResponse {
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            self.status,
            self.reason,
            self.content_type,
            self.body.len()
        )
        .into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

fn text_response(status: u16, content_type: &str, body: &str) -> HttpResponse {
    bytes_response(status, content_type, body.as_bytes().to_vec())
}

fn bytes_response(status: u16, content_type: &str, body: Vec<u8>) -> HttpResponse {
    HttpResponse {
        status,
        reason: reason(status),
        content_type: content_type.to_string(),
        body,
    }
}

fn json_response(status: u16, value: &impl Serialize) -> Result<HttpResponse> {
    Ok(bytes_response(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec_pretty(value)?,
    ))
}

fn json_error(status: u16, message: &str) -> HttpResponse {
    let body = serde_json::json!({ "error": message });
    bytes_response(
        status,
        "application/json; charset=utf-8",
        serde_json::to_vec_pretty(&body)
            .unwrap_or_else(|_| b"{\"error\":\"request failed\"}".to_vec()),
    )
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

#[derive(Debug, Serialize)]
struct BundleIndexResponse {
    root: String,
    bundle_count: usize,
    labeled_count: usize,
    unlabeled_count: usize,
    bundles: Vec<BundleEntry>,
}

#[derive(Debug, Serialize)]
struct BundleEntry {
    bundle_path: String,
    collection: String,
    name: String,
    image_path: String,
    label_path: Option<String>,
    capture_path: Option<String>,
    labeled: bool,
    label_error: Option<String>,
    detection_count: usize,
    ignored_count: usize,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Serialize)]
struct LabelResponse {
    path: Option<String>,
    exists: bool,
    raw: String,
    spec: EvalSpec,
    validation_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DraftDetectionRequest {
    id: String,
    text: String,
    bounds: Rect,
    #[serde(default)]
    bounds_tolerance: Option<BoundsTolerance>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Debug, Serialize)]
struct DraftDetectionResponse {
    detection: ExpectedDetection,
}

#[derive(Debug, Serialize)]
struct DetectionRunResponse {
    result: OcrLookupResult,
    report: Option<EvalReport>,
}

fn scan_eval_bundles(root: &Path, rel_dir: &Path, bundles: &mut Vec<BundleEntry>) -> Result<()> {
    let dir = root.join(rel_dir);
    if let Some(image_path) = find_bundle_image(&dir) {
        bundles.push(summarize_bundle(root, &dir, &image_path)?);
        return Ok(());
    }

    let mut entries = fs::read_dir(&dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type()?.is_dir() {
            scan_eval_bundles(root, &rel_dir.join(name.as_ref()), bundles)?;
        }
    }
    Ok(())
}

fn summarize_bundle(root: &Path, bundle_path: &Path, image_path: &Path) -> Result<BundleEntry> {
    let label_path = label_path_for_image(image_path)?;
    let capture_path = image_path.with_file_name("capture.json");
    let (detection_count, ignored_count, label_error) = if label_path.exists() {
        match eval::load_eval_spec(&label_path) {
            Ok(spec) => (spec.detections.len(), spec.ignored.len(), None),
            Err(error) => (0, 0, Some(error.to_string())),
        }
    } else {
        (0, 0, None)
    };
    let dimensions = image::image_dimensions(image_path).ok();
    Ok(BundleEntry {
        bundle_path: rel_string(root, bundle_path)?,
        collection: bundle_path
            .parent()
            .and_then(|parent| rel_string(root, parent).ok())
            .unwrap_or_default(),
        name: bundle_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        image_path: rel_string(root, image_path)?,
        label_path: label_path
            .exists()
            .then(|| rel_string(root, &label_path))
            .transpose()?,
        capture_path: capture_path
            .exists()
            .then(|| rel_string(root, &capture_path))
            .transpose()?,
        labeled: label_path.exists() && label_error.is_none(),
        label_error,
        detection_count,
        ignored_count,
        width: dimensions.map(|(width, _)| width),
        height: dimensions.map(|(_, height)| height),
    })
}

fn default_eval_spec(image_path: &Path) -> Result<EvalSpec> {
    let capture = image_path.with_file_name("capture.json");
    Ok(EvalSpec {
        schema: 2,
        image: Some(
            image_path
                .file_name()
                .context("image path has no file name")?
                .to_string_lossy()
                .to_string(),
        ),
        source_capture: capture.exists().then(|| "capture.json".to_string()),
        detections: Vec::new(),
        ignored: Vec::new(),
        notes: None,
    })
}

fn label_path_for_image(image_path: &Path) -> Result<PathBuf> {
    Ok(image_path
        .parent()
        .context("image path has no parent directory")?
        .join("eval.json"))
}

fn clean_relative_path(input: &str) -> Result<PathBuf> {
    let input = input.replace('\\', "/");
    let path = Path::new(&input);
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("path must stay under eval root: {input}");
            }
        }
    }
    Ok(clean)
}

fn rel_string(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?;
    Ok(rel
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn ensure_eval_bundle(path: &Path) -> Result<()> {
    if !path.is_dir() {
        bail!("{} is not an eval folder bundle", path.display());
    }
    Ok(())
}

fn find_bundle_image(bundle_path: &Path) -> Option<PathBuf> {
    if !bundle_path.is_dir() {
        return None;
    }
    for name in ["underlying.png", "underlying.jpg", "underlying.jpeg"] {
        let image_path = bundle_path.join(name);
        if image_path.is_file() {
            return Some(image_path);
        }
    }
    None
}

fn image_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => "image/png",
    }
}

fn upload_extension(original_name: &str, body: &[u8]) -> Result<&'static str> {
    let name_ext = Path::new(original_name)
        .file_name()
        .and_then(|name| Path::new(name).extension())
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let guessed = image::guess_format(body).context("uploaded file is not a recognized image")?;
    let ext = match guessed {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        other => bail!("unsupported upload format {other:?}; use PNG or JPEG"),
    };
    if let Some(name_ext) = name_ext
        && !matches!(name_ext.as_str(), "png" | "jpg" | "jpeg")
    {
        bail!("unsupported image extension .{name_ext}; use PNG or JPEG");
    }
    Ok(ext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::{Entry, Sense};

    #[test]
    fn clean_relative_path_rejects_escape_attempts() {
        assert!(clean_relative_path("../secret.png").is_err());
        assert!(clean_relative_path("collection-name/../secret.png").is_err());
        assert!(clean_relative_path("C:/secret.png").is_err());
        assert_eq!(
            clean_relative_path("collection-name/capture/underlying.png").unwrap(),
            PathBuf::from("collection-name")
                .join("capture")
                .join("underlying.png")
        );
    }

    #[test]
    fn percent_decode_handles_utf8_and_spaces() {
        assert_eq!(percent_decode("a+b%20c").unwrap(), "a b c");
        assert_eq!(percent_decode("%E6%AD%A6%E5%99%A8").unwrap(), "武器");
    }

    #[test]
    fn scan_eval_bundles_returns_bundle_paths() {
        let root = std::env::temp_dir().join(format!(
            "mite-eval-ui-test-{}-{}",
            std::process::id(),
            artifact::unix_ms()
        ));
        let capture = root.join("collection-name").join("capture-123");
        fs::create_dir_all(&capture).unwrap();
        fs::write(capture.join("underlying.png"), []).unwrap();
        fs::write(capture.join("with_overlay.png"), []).unwrap();
        fs::write(root.join("loose.png"), []).unwrap();

        let mut bundles = Vec::new();
        scan_eval_bundles(&root, Path::new(""), &mut bundles).unwrap();

        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].bundle_path, "collection-name/capture-123");
        assert_eq!(bundles[0].collection, "collection-name");
        assert_eq!(bundles[0].name, "capture-123");
        assert_eq!(
            bundles[0].image_path,
            "collection-name/capture-123/underlying.png"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn draft_detection_round_trips_through_eval_validation() {
        let dict = Dictionary::from_entries(vec![Entry {
            kanji: vec!["武器".to_string()],
            kana: vec!["ぶき".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["n".to_string()],
                glosses: vec!["weapon".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: None,
        }]);
        let detection = eval::draft_expected_detection(
            &dict,
            "weapon",
            "武器",
            Rect::new(10.0, 20.0, 80.0, 30.0),
            vec![
                Rect::new(10.0, 20.0, 37.0, 30.0),
                Rect::new(47.0, 20.0, 43.0, 30.0),
            ],
            None,
            None,
        )
        .unwrap();
        assert_eq!(detection.characters.len(), 2);
        assert_eq!(
            detection.character_geometry,
            eval::CharacterGeometrySource::PixelGradientV1
        );
        assert_eq!(
            detection.characters[0].bounds,
            Rect::new(10.0, 20.0, 37.0, 30.0)
        );
        assert_eq!(detection.tokens.len(), 1);
        assert_eq!(detection.tokens[0].dictionary_form, "武器");
    }
}
