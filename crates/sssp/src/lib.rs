//! # SSSP project container
//!
//! C3 (`sssp_c3`) saves its documents as a SQLite database: `PRAGMA application_id = 0x53535350`
//! ("SSSP"), `user_version = 1`. This crate reads and writes that container so the two apps can
//! open each other's projects — which matters for the layers Photoshop's PSD has no room for
//! (网点 / halftone, glitch, grid, 屏幕印刷 …): only SSSP keeps them.
//!
//! ## Layout (mirrors `SSSPDocumentContainer.swift`)
//!
//! * `document` — one row, `document_id TEXT`
//! * `heads` — `manual` / `automatic`, each with a generation and a save time
//! * `metadata(head, scope, entity_id, json)` — the project JSON split into entities:
//!   `scope = 'root'` (one row per top-level key, except `layers`/`groups`),
//!   `scope = 'layer'` (one row per layer, keyed by layer id),
//!   `scope = 'group'`, and `scope = 'layout'` holding the id order (`layers`, `groups`)
//! * `tile_payloads(version_id, width, height, tile_size, codec, data)` + `tile_refs` — pixels, cut
//!   into `tile_size` squares per layer, each tile **RGBA16Float, premultiplied, 8 bytes a pixel**
//! * `resource_payloads` + `resource_refs` — big blobs the JSON refers to as
//!   `{"$ssspResource": "<id>"}` (C3 inlines them as base64 when it hands the JSON out)
//! * `previews` — one PNG per head
//! * `color_identity(head, layer_id, x, y, width, height, data)` — 2 bytes a pixel, the hue /
//!   saturation identity, so a restart can still eyedrop a hue out of the artwork
//!
//! Tile compression is `lzfse` when it helps and `raw` otherwise, exactly like `SSSPTileCodec`.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, params};
use serde_json::{Map, Value};

/// `PRAGMA application_id` of an SSSP container: ASCII "SSSP".
pub const APPLICATION_ID: u32 = 0x5353_5350;
/// `PRAGMA user_version` this crate writes, and requires when reading.
pub const SCHEMA_VERSION: i64 = 1;

/// Which of the two heads a container row belongs to. C3 keeps a manual and an automatic save of
/// the same document inside one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    /// The save the user asked for.
    Manual,
    /// The periodic autosave.
    Automatic,
}

impl Head {
    /// The text stored in the `heads.kind` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Head::Manual => "manual",
            Head::Automatic => "automatic",
        }
    }

    /// Parse a `heads.kind` value.
    pub fn from_str(text: &str) -> Option<Self> {
        match text {
            "manual" => Some(Head::Manual),
            "automatic" => Some(Head::Automatic),
            _ => None,
        }
    }
}

/// How a tile's bytes are packed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// RGBA16Float rows, nothing compressed.
    Raw,
    /// The same bytes through LZFSE.
    Lzfse,
}

impl Codec {
    /// The text stored in the `tile_payloads.codec` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Codec::Raw => "raw",
            Codec::Lzfse => "lzfse",
        }
    }

    /// Parse a `tile_payloads.codec` value.
    pub fn from_str(text: &str) -> Option<Self> {
        match text {
            "raw" => Some(Codec::Raw),
            "lzfse" => Some(Codec::Lzfse),
            _ => None,
        }
    }
}

/// One stored tile's payload row (`tile_payloads`). Two references to the same version share it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TilePayload {
    /// Content version id: C3 mints a fresh one per distinct tile content.
    pub version_id: String,
    /// Tile width in pixels, at most `tile_size`.
    pub width: u32,
    /// Tile height in pixels, at most `tile_size`.
    pub height: u32,
    /// The document's tile grid step.
    pub tile_size: u32,
    /// How `bytes` is packed.
    pub codec: Codec,
    /// The payload, as stored.
    pub bytes: Vec<u8>,
}

/// One reference (`tile_refs`): this layer owns a tile at grid cell (`x`, `y`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    /// Layer id the tile belongs to.
    pub layer_id: String,
    /// Tile column in the layer's grid.
    pub x: u32,
    /// Tile row in the layer's grid.
    pub y: u32,
    /// The tile's payload.
    pub payload: TilePayload,
}

/// One `color_identity` tile: 2 bytes of hue / saturation identity per pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorIdentityTile {
    /// Layer id the tile belongs to.
    pub layer_id: String,
    /// Tile column in the layer's grid.
    pub x: u32,
    /// Tile row in the layer's grid.
    pub y: u32,
    /// Tile width in pixels.
    pub width: u32,
    /// Tile height in pixels.
    pub height: u32,
    /// `width * height * 2` bytes.
    pub bytes: Vec<u8>,
}

/// The PNG preview stored for a head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    /// Generation the preview was made for.
    pub generation: u64,
    /// PNG bytes.
    pub bytes: Vec<u8>,
}

/// A whole container, as read or as it is to be written.
#[derive(Clone, Debug)]
pub struct Project {
    /// The project JSON, assembled exactly as C3 assembles it: `layers` and `groups` are arrays in
    /// `layout` order, and blobs stay as `{"$ssspResource": id}` placeholders. Use
    /// [`Project::inline_resources`] to turn those into the base64 strings C3's decoder sees.
    pub metadata: Value,
    /// Every tile reference, in no particular order.
    pub tiles: Vec<Tile>,
    /// Hue / saturation identity tiles.
    pub color_identity: Vec<ColorIdentityTile>,
    /// Blobs the metadata references.
    pub resources: BTreeMap<String, Vec<u8>>,
    /// The preview PNG, if the file has one.
    pub preview: Option<Preview>,
    /// Which head was read, and which head a write fills.
    pub head: Head,
    /// The head's generation. A write keeps its own monotonically increasing value.
    pub generation: u64,
}

/// Everything this crate can fail at.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The SQLite layer failed.
    #[error("sssp container: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// An I/O call failed.
    #[error("sssp io: {0}")]
    Io(#[from] std::io::Error),
    /// The metadata JSON could not be decoded.
    #[error("sssp metadata: {0}")]
    Json(#[from] serde_json::Error),
    /// The file is not an SSSP container.
    #[error("not an SSSP container")]
    NotAContainer,
    /// The container's schema version is one this crate does not know.
    #[error("unsupported SSSP schema version {0}")]
    UnsupportedSchema(i64),
    /// A metadata entity is missing, or a layout names an entity that is not there.
    #[error("invalid SSSP metadata: {0}")]
    Metadata(String),
    /// A tile's dimensions or bytes do not add up.
    #[error("invalid SSSP tile: {0}")]
    Tile(String),
}

/// The result type of this crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Project {
    /// A project with a fresh empty document of the given canvas size.
    pub fn new_document(canvas_width: u32, canvas_height: u32, tile_size: u32) -> Self {
        let mut root = Map::new();
        root.insert("formatName".into(), Value::String("sssp".into()));
        root.insert("formatVersion".into(), Value::from(1));
        root.insert("canvasWidth".into(), Value::from(canvas_width));
        root.insert("canvasHeight".into(), Value::from(canvas_height));
        root.insert("canvasDPI".into(), Value::from(72));
        root.insert("activeLayerID".into(), Value::String(String::new()));
        root.insert("layerOrder".into(), Value::Array(Vec::new()));
        root.insert("layers".into(), Value::Array(Vec::new()));
        root.insert("tileSize".into(), Value::from(tile_size.max(1)));
        Project {
            metadata: Value::Object(root),
            tiles: Vec::new(),
            color_identity: Vec::new(),
            resources: BTreeMap::new(),
            preview: None,
            head: Head::Manual,
            generation: 1,
        }
    }

    /// The tile grid step the metadata records, falling back to 256.
    pub fn tile_size(&self) -> u32 {
        self.metadata
            .get("tileSize")
            .and_then(Value::as_u64)
            .map(|v| u32::try_from(v).unwrap_or(256))
            .filter(|v| *v > 0)
            .unwrap_or(256)
    }

    /// Canvas width from the metadata.
    pub fn canvas_width(&self) -> u32 {
        self.metadata
            .get("canvasWidth")
            .and_then(Value::as_u64)
            .map(|v| u32::try_from(v).unwrap_or(0))
            .unwrap_or(0)
    }

    /// Canvas height from the metadata.
    pub fn canvas_height(&self) -> u32 {
        self.metadata
            .get("canvasHeight")
            .and_then(Value::as_u64)
            .map(|v| u32::try_from(v).unwrap_or(0))
            .unwrap_or(0)
    }

    /// The metadata's `layers` array, top to bottom as C3 lists it.
    pub fn layers(&self) -> &[Value] {
        self.metadata
            .get("layers")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The tiles of one layer, sorted by row then column.
    pub fn tiles_of(&self, layer_id: &str) -> Vec<&Tile> {
        let mut tiles: Vec<&Tile> = self.tiles.iter().filter(|t| t.layer_id == layer_id).collect();
        tiles.sort_by_key(|t| (t.y, t.x));
        tiles
    }

    /// The project JSON with `{"$ssspResource": id}` placeholders replaced by the base64 strings
    /// C3's decoder receives.
    pub fn inline_resources(&self) -> Result<Value> {
        inline(&self.metadata, &self.resources)
    }
}

/// Replace `{"$ssspResource": id}` placeholders with the base64 string C3 hands its decoder.
pub fn inline(value: &Value, resources: &BTreeMap<String, Vec<u8>>) -> Result<Value> {
    match value {
        Value::Object(map) => {
            if let Some(id) = map.get("$ssspResource").and_then(Value::as_str) {
                let bytes = resources
                    .get(id)
                    .ok_or_else(|| Error::Metadata(format!("missing resource {id}")))?;
                return Ok(Value::String(base64_encode(bytes)));
            }
            let mut out = Map::with_capacity(map.len());
            for (key, child) in map {
                out.insert(key.clone(), inline(child, resources)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|item| inline(item, resources))
                .collect::<Result<Vec<_>>>()?,
        )),
        other => Ok(other.clone()),
    }
}

/// The other direction: pull base64 blobs out of the JSON into a resource table, leaving
/// `{"$ssspResource": id}` placeholders behind. Blobs shorter than `min_bytes` stay inline, which
/// is what keeps small strings readable in the metadata.
pub fn extract(value: &Value, min_bytes: usize) -> Result<(Value, BTreeMap<String, Vec<u8>>)> {
    let mut resources = BTreeMap::new();
    fn walk(
        value: &Value,
        min_bytes: usize,
        resources: &mut BTreeMap<String, Vec<u8>>,
        counter: &mut u64,
    ) -> Result<Value> {
        match value {
            Value::Object(map) => {
                let mut out = Map::with_capacity(map.len());
                for (key, child) in map {
                    out.insert(key.clone(), walk(child, min_bytes, resources, counter)?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(items) => Ok(Value::Array(
                items
                    .iter()
                    .map(|item| walk(item, min_bytes, resources, counter))
                    .collect::<Result<Vec<_>>>()?,
            )),
            Value::String(text) if text.len() >= min_bytes && looks_like_base64(text) => {
                let bytes = base64_decode(text)?;
                *counter += 1;
                let id = format!("r{counter:08}");
                resources.insert(id.clone(), bytes);
                let mut placeholder = Map::new();
                placeholder.insert("$ssspResource".into(), Value::String(id));
                Ok(Value::Object(placeholder))
            }
            other => Ok(other.clone()),
        }
    }
    let mut counter = 0;
    let stripped = walk(value, min_bytes, &mut resources, &mut counter)?;
    Ok((stripped, resources))
}

/// Does `path` hold an SSSP container with a schema this crate understands?
pub fn is_sssp(path: impl AsRef<Path>) -> bool {
    match Connection::open_with_flags(
        path.as_ref(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(connection) => application_id(&connection)
            .map(|id| id == APPLICATION_ID)
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Read a container, preferring the manual head and falling back to the other one.
pub fn read(path: impl AsRef<Path>) -> Result<Project> {
    read_head(path, Head::Manual)
}

/// Read a container's specific head.
pub fn read_head(path: impl AsRef<Path>, head: Head) -> Result<Project> {
    let connection = open_read_only(path.as_ref())?;
    let version = user_version(&connection)?;
    if version != SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema(version));
    }
    let heads = read_heads(&connection)?;
    let chosen = heads
        .iter()
        .find(|(kind, _, _)| *kind == head)
        .or_else(|| heads.first())
        .ok_or_else(|| Error::Metadata("container has no heads".into()))?;
    let head = chosen.0;
    let generation = chosen.1;

    let resources = read_resources(&connection, head)?;
    Ok(Project {
        metadata: read_metadata(&connection, head)?,
        tiles: read_tiles(&connection, head)?,
        color_identity: read_color_identity(&connection, head)?,
        preview: read_preview(&connection, head)?,
        resources,
        head,
        generation,
    })
}

/// Read a container handed over as bytes: the bytes are spilled to a temporary SQLite file, read,
/// and removed again, so callers that only have a byte slice (the ordinary import path) can use
/// this too.
pub fn read_bytes(bytes: &[u8]) -> Result<Project> {
    let path = temporary_path("read");
    std::fs::write(&path, bytes)?;
    let out = read(&path);
    let _ = std::fs::remove_file(&path);
    out
}

/// Write `project` into a fresh container and hand back its bytes.
pub fn write_bytes(project: &Project) -> Result<Vec<u8>> {
    let path = temporary_path("write");
    let out = write(&path, project).map_err(Error::from).and_then(|()| std::fs::read(&path).map_err(Error::from));
    let _ = std::fs::remove_file(&path);
    out
}

fn temporary_path(tag: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("sssp-{tag}-{}-{nanos}-{n}.db", std::process::id()))
}

/// The heads (and generations) a container holds, newest first.
pub fn heads(path: impl AsRef<Path>) -> Result<Vec<(Head, u64)>> {
    let mut heads = read_heads(&open_read_only(path.as_ref())?)?;
    heads.sort_by_key(|(_, generation, _)| std::cmp::Reverse(*generation));
    Ok(heads.into_iter().map(|(head, generation, _)| (head, generation)).collect())
}

/// Write `project` as a fresh container at `path`, replacing whatever is there.
///
/// The file is written to a temporary neighbour and renamed into place, so a failed write never
/// leaves half a project behind.
pub fn write(path: impl AsRef<Path>, project: &Project) -> Result<()> {
    let path = path.as_ref();
    let mut temporary = path.to_path_buf();
    temporary.set_extension("sssp-writing");
    let _ = std::fs::remove_file(&temporary);
    match write_inner(&temporary, project) {
        Ok(()) => {
            std::fs::rename(&temporary, path)?;
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn write_inner(path: &Path, project: &Project) -> Result<()> {
    let _ = std::fs::remove_file(path);
    let connection = Connection::open(path)?;
    connection.execute_batch(&format!(
        "PRAGMA application_id={APPLICATION_ID};\n\
         PRAGMA user_version={SCHEMA_VERSION};\n\
         PRAGMA journal_mode=DELETE;\n\
         PRAGMA synchronous=FULL;\n\
         CREATE TABLE document(id INTEGER PRIMARY KEY CHECK(id=1),document_id TEXT NOT NULL);\n\
         CREATE TABLE heads(kind TEXT PRIMARY KEY CHECK(kind IN ('manual','automatic')),generation TEXT NOT NULL,saved_at REAL NOT NULL);\n\
         CREATE TABLE metadata(head TEXT NOT NULL REFERENCES heads(kind) ON DELETE CASCADE,scope TEXT NOT NULL,entity_id TEXT NOT NULL,json BLOB NOT NULL,PRIMARY KEY(head,scope,entity_id)) WITHOUT ROWID;\n\
         CREATE TABLE tile_payloads(version_id TEXT PRIMARY KEY,width INTEGER NOT NULL,height INTEGER NOT NULL,tile_size INTEGER NOT NULL,codec TEXT NOT NULL,data BLOB NOT NULL);\n\
         CREATE TABLE tile_refs(head TEXT NOT NULL REFERENCES heads(kind) ON DELETE CASCADE, layer_id TEXT NOT NULL, x INTEGER NOT NULL, y INTEGER NOT NULL, version_id TEXT NOT NULL REFERENCES tile_payloads(version_id), PRIMARY KEY(head,layer_id,x,y)) WITHOUT ROWID;\n\
         CREATE INDEX tile_refs_payload ON tile_refs(version_id);\n\
         CREATE TABLE resource_payloads(payload_id TEXT PRIMARY KEY,data BLOB NOT NULL);\n\
         CREATE TABLE resource_refs(head TEXT NOT NULL REFERENCES heads(kind) ON DELETE CASCADE,resource_id TEXT NOT NULL,payload_id TEXT NOT NULL REFERENCES resource_payloads(payload_id),PRIMARY KEY(head,resource_id)) WITHOUT ROWID;\n\
         CREATE INDEX resource_refs_payload ON resource_refs(payload_id);\n\
         CREATE TABLE previews(head TEXT PRIMARY KEY REFERENCES heads(kind) ON DELETE CASCADE,generation TEXT NOT NULL,data BLOB NOT NULL);\n\
         CREATE TABLE color_identity(head TEXT NOT NULL, layer_id TEXT NOT NULL, x INTEGER NOT NULL, y INTEGER NOT NULL, width INTEGER NOT NULL, height INTEGER NOT NULL, data BLOB NOT NULL, PRIMARY KEY(head,layer_id,x,y)) WITHOUT ROWID;"
    ))?;

    let head = project.head.as_str();
    let document_id = project
        .metadata
        .get("documentID")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(new_document_id);
    connection.execute(
        "INSERT INTO document(id, document_id) VALUES(1, ?)",
        params![document_id],
    )?;
    let saved_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    connection.execute(
        "INSERT INTO heads(kind, generation, saved_at) VALUES(?, ?, ?)",
        params![head, project.generation.to_string(), saved_at],
    )?;

    let entities = split_metadata(&project.metadata)?;
    for (scope, entity, value) in entities {
        connection.execute(
            "INSERT INTO metadata(head, scope, entity_id, json) VALUES(?, ?, ?, ?)",
            params![head, scope, entity, serde_json::to_vec(&value)?],
        )?;
    }
    for tile in &project.tiles {
        connection
            .execute(
                "INSERT OR IGNORE INTO tile_payloads(version_id, width, height, tile_size, codec, data) \
                 VALUES(?, ?, ?, ?, ?, ?)",
                params![
                    &tile.payload.version_id,
                    tile.payload.width as i64,
                    tile.payload.height as i64,
                    tile.payload.tile_size as i64,
                    tile.payload.codec.as_str(),
                    &tile.payload.bytes
                ],
            )
            .or_else(|error| match error {
                rusqlite::Error::SqliteFailure(_, _) => Ok(0),
                other => Err(other),
            })?;
        connection.execute(
            "INSERT OR REPLACE INTO tile_refs(head, layer_id, x, y, version_id) VALUES(?, ?, ?, ?, ?)",
            params![
                head,
                tile.layer_id,
                tile.x as i64,
                tile.y as i64,
                &tile.payload.version_id
            ],
        )?;
    }
    for (resource_id, bytes) in &project.resources {
        let payload_id = payload_id_of(bytes);
        connection
            .execute(
                "INSERT OR IGNORE INTO resource_payloads(payload_id, data) VALUES(?, ?)",
                params![payload_id, bytes],
            )
            .or_else(|error| match error {
                rusqlite::Error::SqliteFailure(_, _) => Ok(0),
                other => Err(other),
            })?;
        connection.execute(
            "INSERT OR REPLACE INTO resource_refs(head, resource_id, payload_id) VALUES(?, ?, ?)",
            params![head, resource_id, payload_id],
        )?;
    }
    if let Some(preview) = &project.preview {
        connection.execute(
            "INSERT INTO previews(head, generation, data) VALUES(?, ?, ?)",
            params![head, preview.generation.to_string(), &preview.bytes],
        )?;
    }
    for tile in &project.color_identity {
        if tile.bytes.len() != tile.width as usize * tile.height as usize * 2 {
            continue;
        }
        connection.execute(
            "INSERT OR REPLACE INTO color_identity(head, layer_id, x, y, width, height, data) \
             VALUES(?, ?, ?, ?, ?, ?, ?)",
            params![
                head,
                tile.layer_id,
                tile.x as i64,
                tile.y as i64,
                tile.width as i64,
                tile.height as i64,
                &tile.bytes
            ],
        )?;
    }
    Ok(())
}

/// The (scope, entity id, json) rows C3 stores for a project: everything but `layers`/`groups`
/// becomes a `root` row, each layer and group becomes its own row, and two `layout` rows hold the
/// id order.
fn split_metadata(metadata: &Value) -> Result<Vec<(&'static str, String, Value)>> {
    let object = metadata
        .as_object()
        .ok_or_else(|| Error::Metadata("metadata is not an object".into()))?;
    let mut out = Vec::new();
    for (key, value) in object {
        if key == "layers" || key == "groups" {
            continue;
        }
        out.push(("root", key.clone(), value.clone()));
    }
    for (array_key, scope) in [("layers", "layer"), ("groups", "group")] {
        let Some(items) = object.get(array_key) else {
            continue;
        };
        let items = items
            .as_array()
            .ok_or_else(|| Error::Metadata(format!("{array_key} is not an array")))?;
        let mut order = Vec::with_capacity(items.len());
        for item in items {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Metadata(format!("{array_key} entry has no id")))?
                .to_owned();
            order.push(Value::String(id.clone()));
            out.push((scope, id, item.clone()));
        }
        out.push(("layout", array_key.to_owned(), Value::Array(order)));
    }
    Ok(out)
}

fn new_document_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:016x}-sssp-{:08x}", std::process::id())
}

/// A stable id for a resource blob: its content decides it, so identical blobs share a payload row.
fn payload_id_of(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}{:08x}", bytes.len() as u64)
}

/// Pack a tile the way `SSSPTileCodec` does: LZFSE when it actually saves bytes, `raw` otherwise.
///
/// `raw` is `width * height * 8` bytes of premultiplied RGBA, half floats, row by row.
pub fn encode_tile(
    version_id: impl Into<String>,
    width: u32,
    height: u32,
    tile_size: u32,
    raw: &[u8],
) -> Result<TilePayload> {
    let expected = raw_tile_bytes(width, height)?;
    if raw.len() != expected {
        return Err(Error::Tile(format!(
            "expected {expected} bytes for a {width}x{height} tile, got {}",
            raw.len()
        )));
    }
    let mut compressed = Vec::new();
    let codec = match lzfse_rust::encode_bytes(raw, &mut compressed) {
        Ok(0) if !compressed.is_empty() && compressed.len() < raw.len() => Codec::Lzfse,
        _ => {
            compressed.clear();
            compressed.extend_from_slice(raw);
            Codec::Raw
        }
    };
    Ok(TilePayload {
        version_id: version_id.into(),
        width,
        height,
        tile_size,
        codec,
        bytes: compressed,
    })
}

/// Unpack a tile back to `width * height * 8` bytes of premultiplied RGBA half floats.
pub fn decode_tile(payload: &TilePayload) -> Result<Vec<u8>> {
    let expected = raw_tile_bytes(payload.width, payload.height)?;
    match payload.codec {
        Codec::Raw => {
            if payload.bytes.len() != expected {
                return Err(Error::Tile(format!(
                    "raw tile is {} bytes, expected {expected}",
                    payload.bytes.len()
                )));
            }
            Ok(payload.bytes.clone())
        }
        Codec::Lzfse => {
            let mut raw = Vec::with_capacity(expected);
            lzfse_rust::decode_bytes(&payload.bytes, &mut raw)
                .map_err(|error| Error::Tile(format!("lzfse: {error}")))?;
            if raw.len() != expected {
                return Err(Error::Tile(format!(
                    "decoded tile is {} bytes, expected {expected}",
                    raw.len()
                )));
            }
            Ok(raw)
        }
    }
}

/// The byte count of an uncompressed `width` x `height` tile: 4 half-float channels.
pub fn raw_tile_bytes(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(Error::Tile("empty tile".into()));
    }
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(8))
        .map(|bytes| bytes as usize)
        .ok_or_else(|| Error::Tile("tile dimensions overflow".into()))
}

/// Cut one layer's pixels into grid tiles.
///
/// `pixels` is `width * height * 8` bytes of premultiplied RGBA half floats. Tiles that are
/// entirely transparent are skipped, exactly as C3 skips empty tiles.
pub fn split_layer(
    layer_id: &str,
    width: u32,
    height: u32,
    tile_size: u32,
    pixels: &[u8],
    version_seed: &str,
) -> Result<Vec<Tile>> {
    let stride = width as usize * 8;
    if pixels.len() != height as usize * stride {
        return Err(Error::Tile("layer pixels do not match its size".into()));
    }
    let step = tile_size.max(1);
    let mut out = Vec::new();
    let mut index = 0u32;
    let mut y = 0;
    while y < height {
        let tile_height = step.min(height - y);
        let mut x = 0;
        while x < width {
            let tile_width = step.min(width - x);
            let mut raw = Vec::with_capacity(tile_width as usize * tile_height as usize * 8);
            for row in 0..tile_height {
                let start = (y + row) as usize * stride + x as usize * 8;
                raw.extend_from_slice(&pixels[start..start + tile_width as usize * 8]);
            }
            let empty = raw.chunks_exact(8).all(|pixel| pixel[6..8] == [0, 0]);
            if !empty {
                index += 1;
                out.push(Tile {
                    layer_id: layer_id.to_owned(),
                    x: x / step,
                    y: y / step,
                    payload: encode_tile(
                        format!("{version_seed}-{index}"),
                        tile_width,
                        tile_height,
                        step,
                        &raw,
                    )?,
                });
            }
            x += step;
        }
        y += step;
    }
    Ok(out)
}

/// Assemble a layer's tiles back into `width * height * 8` bytes of premultiplied RGBA half floats.
pub fn assemble_layer(width: u32, height: u32, tiles: &[&Tile]) -> Result<Vec<u8>> {
    let stride = width as usize * 8;
    let mut out = vec![0u8; height as usize * stride];
    for tile in tiles {
        let raw = decode_tile(&tile.payload)?;
        let step = tile.payload.tile_size.max(1) as usize;
        let x0 = tile.x as usize * step;
        let y0 = tile.y as usize * step;
        for row in 0..tile.payload.height as usize {
            let source = row * tile.payload.width as usize * 8;
            let target = (y0 + row) * stride + x0 * 8;
            if x0 + tile.payload.width as usize > width as usize
                || y0 + row >= height as usize
                || source + tile.payload.width as usize * 8 > raw.len()
                || target + tile.payload.width as usize * 8 > out.len()
            {
                return Err(Error::Tile(format!("tile falls outside the layer: tile ({},{}) {}x{}, tile_size {}, layer {}x{}", tile.x, tile.y, tile.payload.width, tile.payload.height, step, width, height)));
            }
            out[target..target + tile.payload.width as usize * 8]
                .copy_from_slice(&raw[source..source + tile.payload.width as usize * 8]);
        }
    }
    Ok(out)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    if application_id(&connection)? != APPLICATION_ID {
        return Err(Error::NotAContainer);
    }
    Ok(connection)
}

fn application_id(connection: &Connection) -> Result<u32> {
    let id: i64 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    Ok(u32::try_from(id).unwrap_or(0))
}

fn user_version(connection: &Connection) -> Result<i64> {
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

fn read_heads(connection: &Connection) -> Result<Vec<(Head, u64, f64)>> {
    let mut statement = connection.prepare("SELECT kind, generation, saved_at FROM heads")?;
    let mut rows = statement.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let generation: String = row.get(1)?;
        let saved_at: f64 = row.get(2)?;
        let head = Head::from_str(&kind)
            .ok_or_else(|| Error::Metadata(format!("unknown head {kind}")))?;
        let generation = generation
            .parse::<u64>()
            .map_err(|_| Error::Metadata(format!("bad generation {generation}")))?;
        out.push((head, generation, saved_at));
    }
    Ok(out)
}

fn read_metadata(connection: &Connection, head: Head) -> Result<Value> {
    let mut root = Map::new();
    let mut layers: BTreeMap<String, Value> = BTreeMap::new();
    let mut groups: BTreeMap<String, Value> = BTreeMap::new();
    let mut layouts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    {
        let mut statement =
            connection.prepare("SELECT scope, entity_id, json FROM metadata WHERE head=?")?;
        let mut rows = statement.query(params![head.as_str()])?;
        while let Some(row) = rows.next()? {
            let scope: String = row.get(0)?;
            let entity: String = row.get(1)?;
            let json: Vec<u8> = row.get(2)?;
            let value: Value = serde_json::from_slice(&json)?;
            match scope.as_str() {
                "root" => {
                    root.insert(entity, value);
                }
                "layer" => {
                    layers.insert(entity, value);
                }
                "group" => {
                    groups.insert(entity, value);
                }
                "layout" => {
                    let ids = value
                        .as_array()
                        .ok_or_else(|| Error::Metadata("layout is not an array".into()))?
                        .iter()
                        .map(|item| {
                            item.as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| Error::Metadata("layout id is not a string".into()))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    layouts.insert(entity, ids);
                }
                other => return Err(Error::Metadata(format!("unknown scope {other}"))),
            }
        }
    }
    if let Some(order) = layouts.get("layers") {
        let mut ordered = Vec::with_capacity(order.len());
        for id in order {
            let layer = layers
                .remove(id)
                .ok_or_else(|| Error::Metadata(format!("layout names missing layer {id}")))?;
            ordered.push(layer);
        }
        root.insert("layers".into(), Value::Array(ordered));
    }
    if let Some(order) = layouts.get("groups") {
        let mut ordered = Vec::with_capacity(order.len());
        for id in order {
            let group = groups
                .remove(id)
                .ok_or_else(|| Error::Metadata(format!("layout names missing group {id}")))?;
            ordered.push(group);
        }
        root.insert("groups".into(), Value::Array(ordered));
    }
    Ok(Value::Object(root))
}

fn read_tiles(connection: &Connection, head: Head) -> Result<Vec<Tile>> {
    let mut statement = connection.prepare(
        "SELECT r.layer_id, r.x, r.y, p.version_id, p.width, p.height, p.tile_size, p.codec, p.data \
         FROM tile_refs r JOIN tile_payloads p ON p.version_id = r.version_id WHERE r.head=?",
    )?;
    let mut rows = statement.query(params![head.as_str()])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let layer_id: String = row.get(0)?;
        let x: i64 = row.get(1)?;
        let y: i64 = row.get(2)?;
        let version_id: String = row.get(3)?;
        let width: i64 = row.get(4)?;
        let height: i64 = row.get(5)?;
        let tile_size: i64 = row.get(6)?;
        let codec: String = row.get(7)?;
        let bytes: Vec<u8> = row.get(8)?;
        out.push(Tile {
            layer_id,
            x: u32::try_from(x).map_err(|_| Error::Tile("negative x".into()))?,
            y: u32::try_from(y).map_err(|_| Error::Tile("negative y".into()))?,
            payload: TilePayload {
                version_id,
                width: u32::try_from(width).map_err(|_| Error::Tile("bad width".into()))?,
                height: u32::try_from(height).map_err(|_| Error::Tile("bad height".into()))?,
                tile_size: u32::try_from(tile_size).map_err(|_| Error::Tile("bad tile size".into()))?,
                codec: Codec::from_str(&codec)
                    .ok_or_else(|| Error::Tile(format!("unknown codec {codec}")))?,
                bytes,
            },
        });
    }
    Ok(out)
}

fn read_resources(connection: &Connection, head: Head) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut statement = connection.prepare(
        "SELECT r.resource_id, p.data FROM resource_refs r \
         JOIN resource_payloads p ON p.payload_id = r.payload_id WHERE r.head=?",
    )?;
    let mut rows = statement.query(params![head.as_str()])?;
    let mut out = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let bytes: Vec<u8> = row.get(1)?;
        out.insert(id, bytes);
    }
    Ok(out)
}

fn read_preview(connection: &Connection, head: Head) -> Result<Option<Preview>> {
    let mut statement =
        connection.prepare("SELECT generation, data FROM previews WHERE head=?")?;
    let mut rows = statement.query(params![head.as_str()])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let generation: String = row.get(0)?;
    let bytes: Vec<u8> = row.get(1)?;
    Ok(Some(Preview {
        generation: generation.parse::<u64>().unwrap_or(0),
        bytes,
    }))
}

fn read_color_identity(
    connection: &Connection,
    head: Head,
) -> Result<Vec<ColorIdentityTile>> {
    let mut out = Vec::new();
    let has_table: bool = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='color_identity'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|_: i64| true)
        .unwrap_or(false);
    if !has_table {
        return Ok(out);
    }
    let mut statement = connection.prepare(
        "SELECT layer_id, x, y, width, height, data FROM color_identity WHERE head=?",
    )?;
    let mut rows = statement.query(params![head.as_str()])?;
    while let Some(row) = rows.next()? {
        let layer_id: String = row.get(0)?;
        let x: i64 = row.get(1)?;
        let y: i64 = row.get(2)?;
        let width: i64 = row.get(3)?;
        let height: i64 = row.get(4)?;
        let bytes: Vec<u8> = row.get(5)?;
        out.push(ColorIdentityTile {
            layer_id,
            x: u32::try_from(x).unwrap_or(0),
            y: u32::try_from(y).unwrap_or(0),
            width: u32::try_from(width).unwrap_or(0),
            height: u32::try_from(height).unwrap_or(0),
            bytes,
        });
    }
    Ok(out)
}

/// Base64, standard alphabet with padding — the encoding C3's `imageDataBase64` uses.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3F] as char
        } else {
            '='
        });
    }
    out
}

/// Decode the standard base64 alphabet, rejecting anything that is not base64.
pub fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(Error::Metadata("base64 length is not a multiple of 4".into()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut values = [0u32; 4];
        let mut pad = 0;
        for (index, byte) in chunk.iter().enumerate() {
            values[index] = match byte {
                b'A'..=b'Z' => u32::from(byte - b'A'),
                b'a'..=b'z' => u32::from(byte - b'a') + 26,
                b'0'..=b'9' => u32::from(byte - b'0') + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => {
                    pad += 1;
                    0
                }
                _ => return Err(Error::Metadata("not base64".into())),
            };
        }
        let triple = (values[0] << 18) | (values[1] << 12) | (values[2] << 6) | values[3];
        out.push((triple >> 16) as u8);
        if pad < 2 {
            out.push((triple >> 8) as u8);
        }
        if pad < 1 {
            out.push(triple as u8);
        }
    }
    Ok(out)
}

fn looks_like_base64(text: &str) -> bool {
    text.len() % 4 == 0
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for sample in [b"".as_slice(), b"a", b"ab", b"abc", b"abcdef", &[0u8, 255, 128]] {
            let encoded = base64_encode(sample);
            assert_eq!(base64_decode(&encoded).expect("decode"), sample.to_vec());
        }
    }

    #[test]
    fn colour_tile_identity_bytes_are_two_a_pixel() {
        let project = Project::new_document(64, 64, 256);
        assert_eq!(project.canvas_width(), 64);
        assert_eq!(project.tile_size(), 256);
        assert!(project.tiles_of("nope").is_empty());
    }
}
