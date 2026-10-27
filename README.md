# Payload Platform

A translation layer that runs on the drone, between one camera payload and the ground control station (GCS). The GCS controls every supported camera through the same API, and the platform translates each request into the attached camera's own protocol.

Supported cameras, simulated in [`camera-simulations/`](camera-simulations/README.md), with vendor documents in [`specs/`](specs/):

| Camera | Protocol | Simulator port |
| --- | --- | --- |
| Brightmere BX-20 | MAVLink 2 camera protocol | 14580 |
| Optronique Vallerin VLR-10 | MAVLink 2 camera protocol | 14581 |
| Lucen LC-30T | Vendor ASCII protocol over UDP | 9003 |

> **Status:** design draft. The application will live in `platform/` and is not written yet.

## Design

### Scope

In scope:

- One camera per platform instance, chosen at start-up with a URI.
- Camera identity, connection status, zoom, active sensor (visible or thermal) and thermal palette.
- One API for the GCS that behaves the same for every camera and works over an IP link between drone and ground.

Out of scope: video streaming, gimbal control, photo and video capture, focus, several cameras at once, and authentication or encryption of the GCS link.

### Approach

The design starts from a per-camera inventory of the three vendor documents and simulators: how each camera is identified, how it zooms, switches sensor and changes palette, how it confirms a change, and how to tell that it is alive. The API offers the common set of those operations in camera-independent units, plus a description of what the attached camera supports. Everything camera-specific lives in a driver.

### Architecture

```
          GCS (on the ground)
                   │  HTTP + JSON over the drone–ground IP link
┌──────────────────▼───────────────────────────────────┐
│ API layer        HTTP routes, JSON, error mapping    │
│ Payload core     validation, state cache, polling,   │
│                  command queue, events               │
│ ── driver contract ───────────────────────────────── │
│ mavlink driver   profiles: bx20, vlr10               │
│ lc30t driver                                         │
└────────┬───────────────────────────────┬─────────────┘
         │ MAVLink 2 over UDP            │ ASCII frames over UDP
   BX-20 or VLR-10                    LC-30T
```

- **API layer** knows HTTP and JSON, and nothing about cameras.
- **Payload core** knows the generic model. It checks every request against the attached camera's descriptors, runs commands one at a time, keeps the last observed state, polls the camera and publishes changes.
- **Drivers** know one protocol each. A driver turns generic operations into camera messages and back, and decides how to confirm a change and how to detect that the camera is alive. Both MAVLink cameras share one driver; their differences live in small per-model profiles.

### Payload model

The GCS sees a payload as **identity + connection + properties + actions**, the same split the W3C Web of Things uses to describe devices. The GCS reads this description once and then knows what the attached camera can do. Nothing in the API is specific to one camera model.

**Identity**

| Field | Example | Source |
| --- | --- | --- |
| `kind` | `camera` | the driver |
| `vendor`, `model` | `Brightmere`, `BX-20` | `CAMERA_INFORMATION` on MAVLink; fixed by the driver for the LC-30T, which can't report it |
| `firmware` | `1.0.0.0` | `CAMERA_INFORMATION`; `null` when unknown |
| `driver`, `profile` | `mavlink`, `bx20` | which driver and profile are in use |
| `uri` | `mavlink://127.0.0.1:14580` | the configured camera URI |

**Connection:** `status` is `connecting`, `online` or `offline`, and `last_seen` is when the camera was last heard from.

**Property descriptor**

| Field | Meaning |
| --- | --- |
| `type` | `number` or `enum` |
| `unit` | numbers only, e.g. `x` for magnification |
| `minimum`, `maximum` | numbers only |
| `resolution` | numbers only: the smallest change the camera can make; requests are rounded to it |
| `options` | enums only: the values this camera accepts |
| `writable` | whether the GCS may set it |
| `value`, `observed_at` | the last value read from the camera, and when it was read |

**Camera vocabulary.** Every camera driver uses these ids, so a GCS can build one camera UI for all of them.

| Id | Kind | Type | Meaning |
| --- | --- | --- | --- |
| `zoom` | property | number, unit `x` | magnification; 1.0 is fully zoomed out |
| `sensor` | property | enum: `visible`, `thermal` | which sensor feeds the video |
| `thermal_palette` | property | enum, see below | colour palette of the thermal image |
| `zoom_step` | action | `direction`: `in` or `out` | zoom one step of 1× |

| | BX-20 | VLR-10 | LC-30T |
| --- | --- | --- | --- |
| `zoom` range | 1–20× | 1–10× | 1–30× |
| `zoom` resolution | 0.1× | 1× | 0.1× |
| `sensor` options | visible, thermal | visible, thermal | visible, thermal |
| `thermal_palette` options | 4 | 4 | 10 |

**Palette names** and their codes on each camera:

| `thermal_palette` | BX-20 and VLR-10 `IR_PALETTE` | LC-30T `IMG` |
| --- | --- | --- |
| `white_hot` | 0 | 00 |
| `black_hot` | 1 | 09 |
| `ironbow` | 2 | 02 |
| `rainbow` | 3 | 06 |
| `lava` | – | 01 |
| `hot_iron` | – | 03 |
| `medical` | – | 04 |
| `arctic` | – | 05 |
| `rainbow_high_contrast` | – | 07 |
| `red_hot` | – | 08 |

**Units and conventions**

- Zoom is always a magnification factor, never a percentage. Percentages don't compare across cameras: half the range is 10.5× on the BX-20, 5.5× on the VLR-10 and 15.5× on the LC-30T. A GCS that wants a slider computes the position from `minimum` and `maximum`.
- `zoom` is the zoom of the visible lens. The thermal image follows it on the VLR-10 and stays fixed on the BX-20; the LC-30T document doesn't say.
- Enum values and JSON field names are `snake_case`.
- Times are RFC 3339 in UTC with milliseconds, e.g. `2026-10-26T14:05:12.480Z`.

### GCS API

HTTP/1.1 with JSON bodies, on port 8080 (configurable), under `/v1`.

Why HTTP + JSON for a drone–ground link:

- Any link that carries IP (mesh radio, LTE, Wi-Fi) carries HTTP, every GCS stack can call it, and it is easy to demonstrate with `curl`.
- Control messages are a few hundred bytes, so JSON's size doesn't matter.
- TCP retransmits lost packets. The price is latency spikes on a bad link, so the API is built to tolerate them: writes can be retried safely, every request has a deadline, and reads are answered from a cache on the drone.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/v1/payload` | identity, connection, every property with its descriptor and value, every action |
| `GET` | `/v1/payload/properties/{id}` | one property |
| `PUT` | `/v1/payload/properties/{id}` | set a property; body `{"value": …}` |
| `POST` | `/v1/payload/actions/{id}` | run an action; the body holds its arguments |
| `GET` | `/v1/payload/events` | server-sent events for every change (stretch goal) |

`GET /v1/payload` with the BX-20 attached:

```json
{
  "identity": {
    "kind": "camera",
    "vendor": "Brightmere",
    "model": "BX-20",
    "firmware": "1.0.0.0",
    "driver": "mavlink",
    "profile": "bx20",
    "uri": "mavlink://127.0.0.1:14580"
  },
  "connection": { "status": "online", "last_seen": "2026-10-26T14:05:12.480Z" },
  "properties": {
    "zoom": {
      "type": "number", "unit": "x", "minimum": 1.0, "maximum": 20.0, "resolution": 0.1,
      "writable": true, "value": 10.5, "observed_at": "2026-10-26T14:05:12.310Z"
    },
    "sensor": {
      "type": "enum", "options": ["visible", "thermal"],
      "writable": true, "value": "visible", "observed_at": "2026-10-26T14:05:12.330Z"
    },
    "thermal_palette": {
      "type": "enum", "options": ["white_hot", "black_hot", "ironbow", "rainbow"],
      "writable": true, "value": "white_hot", "observed_at": "2026-10-26T14:05:12.350Z"
    }
  },
  "actions": {
    "zoom_step": { "parameters": { "direction": { "type": "enum", "options": ["in", "out"] } } }
  }
}
```

Setting a property returns it as the camera reports it afterwards:

```http
PUT /v1/payload/properties/zoom
{"value": 4}

200 OK
{"type": "number", "unit": "x", "minimum": 1.0, "maximum": 20.0, "resolution": 0.1,
 "writable": true, "value": 4.0, "observed_at": "2026-10-26T14:05:13.020Z"}
```

Running an action returns the properties it changed:

```http
POST /v1/payload/actions/zoom_step
{"direction": "in"}

200 OK
{"properties": {"zoom": {"value": 5.0, "observed_at": "2026-10-26T14:05:13.410Z"}}}
```

Rules the GCS can rely on:

- A write returns only after the camera has confirmed it, with the value the camera reports afterwards.
- Every request is checked against the descriptor before anything is sent to the camera. Enum values must be in `options`. Numbers outside `minimum`–`maximum` are refused; numbers inside are rounded to `resolution`, and the reply shows the result.
- `PUT` is idempotent, so the GCS may resend it after a timeout. `zoom_step` is not: the platform never retries it, and the GCS should re-read `zoom` before trying again.
- Commands run one at a time, in arrival order. Reads never wait for commands: they are answered from the cache, which is refreshed every second and right after each command. `observed_at` says how fresh each value is.
- Every request finishes within 5 s.
- New properties and actions don't change the API version. Breaking changes would go to `/v2`.

**Events (stretch goal).** `GET /v1/payload/events` is a server-sent events stream. It starts with a `snapshot` event holding the full `/v1/payload` body, then sends a `property` event for each changed value and a `connection` event when the status changes, with a keep-alive comment every 15 s. A client that reconnects gets a fresh snapshot, so a dropped link never leaves the GCS with a wrong picture.

### Errors

Every error has the same body:

```json
{
  "error": {
    "code": "out_of_range",
    "message": "zoom must be between 1.0 and 20.0",
    "details": { "property": "zoom", "minimum": 1.0, "maximum": 20.0 }
  }
}
```

| `code` | HTTP | Meaning | What the GCS should do |
| --- | --- | --- | --- |
| `invalid_request` | 400 | the body isn't valid JSON, a field has the wrong type, or an argument is unknown | fix the request |
| `not_found` | 404 | this payload has no property or action with that id | check `GET /v1/payload` |
| `read_only` | 405 | the property isn't writable | – |
| `unsupported_value` | 422 | an enum value this camera doesn't offer, e.g. `lava` on the BX-20 | pick from `options` |
| `out_of_range` | 422 | a number outside `minimum`–`maximum` | stay within the range |
| `rejected` | 409 | the camera refused the command | re-read the property; don't retry blindly |
| `payload_offline` | 503 | the camera isn't connected; nothing was sent | wait until `connection.status` is `online` |
| `busy` | 503 | too many commands queued, or the camera asked for a retry | retry after a moment |
| `timeout` | 504 | sent but not confirmed in time; it may or may not have happened | re-read the property before retrying |
| `internal` | 500 | a bug, or a camera reply the platform doesn't understand | report it |

How camera answers map to these codes:

| Camera answer | Result |
| --- | --- |
| `MAV_RESULT_ACCEPTED`, `PARAM_ACK_ACCEPTED`, or an LC-30T read-back equal to the request | success |
| `MAV_RESULT_IN_PROGRESS`, `PARAM_ACK_IN_PROGRESS` | keep waiting |
| `MAV_RESULT_DENIED`, `MAV_RESULT_FAILED`, `PARAM_ACK_VALUE_UNSUPPORTED`, `PARAM_ACK_FAILED`, or an LC-30T read-back that differs | `rejected` |
| `MAV_RESULT_TEMPORARILY_REJECTED` | `busy` |
| `MAV_RESULT_UNSUPPORTED` | `internal`: the profile sent something this camera can't do |
| no answer after every retry | `timeout` |

### Camera URI

The platform is started with the URI of its one camera:

```
platform --camera <uri> [--listen 0.0.0.0:8080]
```

```
<scheme>://<host>:<port>[?<option>=<value>[&…]]
```

The scheme picks the driver. Everything after `://` belongs to that driver, so a new driver can define its own form, for example a serial device path, without changing the core.

| Scheme | Driver | Options |
| --- | --- | --- |
| `mavlink` | MAVLink 2 camera over UDP | `profile`: `auto` (default), `bx20` or `vlr10`; `system_id` (default 1) and `component_id` (default 100) of the camera |
| `lc30t` | Lucen LC-30T ASCII protocol over UDP | `interval_ms`: minimum gap between frames (default 50) |

| URI | Camera |
| --- | --- |
| `mavlink://127.0.0.1:14580` | BX-20 simulator; model detected |
| `mavlink://127.0.0.1:14581` | VLR-10 simulator; model detected |
| `mavlink://127.0.0.1:14581?profile=vlr10` | the same, with the profile forced |
| `lc30t://127.0.0.1:9003` | LC-30T simulator |
| `lc30t://192.168.144.108:9003` | a real LC-30T at its factory address |

With `profile=auto`, the MAVLink driver requests `CAMERA_INFORMATION` and picks the profile from `vendor_name` and `model_name`. An unknown model stops start-up with an error that lists the known models; `profile=` overrides the detection.

### Translation per camera

| | BX-20 | VLR-10 | LC-30T |
| --- | --- | --- | --- |
| Identity | `MAV_CMD_REQUEST_MESSAGE` 259 → `CAMERA_INFORMATION` | same | fixed in the driver: Lucen LC-30T, firmware unknown |
| Firmware | `0x00010000`, documented as 1.0.0.0 | `0x02030000`, documented as 2.3 | – |
| Read `zoom` | request 260 → `CAMERA_SETTINGS.zoomLevel` is a percentage; factor = 1 + 0.19 × level | request 260 → `zoomLevel` is the factor | `MUL` read, in tenths |
| Write `zoom` | `MAV_CMD_SET_CAMERA_ZOOM` RANGE, level = (factor − 1) / 19 × 100 | no absolute zoom: read the factor, then send the difference as single steps | `MUL` write of factor × 10, then `MUL` read-back |
| `zoom_step` | `MAV_CMD_SET_CAMERA_ZOOM` STEP ±1 | same | `MUL` read, write ±10 tenths, read back |
| Read `sensor` | request 270 → `VIDEO_STREAM_STATUS` THERMAL flag | request 270 → which of streams 1 and 2 is RUNNING | `PIP` read: 00 or 03 |
| Write `sensor` | `MAV_CMD_SET_CAMERA_SOURCE`: param1 0, param2 1 or 2, param3 0 | `MAV_CMD_VIDEO_START_STREAMING`: param1 1 or 2 | `PIP` write 00 or 03, then read-back |
| `thermal_palette` | `PARAM_EXT_REQUEST_READ` and `PARAM_EXT_SET` of `IR_PALETTE` (uint8) | same | `IMG` read and write, then read-back |
| Confirmation | `COMMAND_ACK` or `PARAM_EXT_ACK`, then a read | same | read-back only; writes never get a reply |
| Alive when | a camera `HEARTBEAT` arrived in the last 3 s | same | a poll reply arrived in the last 3 s |

Neither MAVLink camera encodes `firmware_version` in the byte order the MAVLink standard defines, so each profile decodes it the way its vendor document describes.

### Timing and liveness

| Setting | Default | Reason |
| --- | --- | --- |
| Platform heartbeat to MAVLink cameras | 1 Hz, as system 1, component 191 (`MAV_COMP_ID_ONBOARD_COMPUTER`) | the camera only talks to addresses it has heard from |
| Camera counted offline after | 3 s without a heartbeat (MAVLink) or a poll reply (LC-30T) | three missed beats |
| State polling | every 1 s | keeps the cache fresh; this traffic stays on the drone's local network |
| Camera request timeout | 500 ms per attempt, 3 attempts | MAVLink resends increment `confirmation`, as the command protocol specifies |
| Retries of `zoom_step` | none | not idempotent; re-read instead |
| Gap between LC-30T frames | at least 50 ms | the vendor's recommendation |
| LC-30T start-up | `connecting` until it answers | the real pod needs about 8 s after power-on |
| API request deadline | 5 s | after that the GCS gets `timeout` |
| Command queue | 16 waiting commands | more than that gets `busy` |

### Driver contract

Every driver, built in or added later, offers the same operations to the core:

- **connect** with its URI: returns the identity and the property and action descriptors, or fails with a clear message.
- **read**: returns the current value of every property; used for polling.
- **write** a property: returns the value the camera reports afterwards, or one of the errors above.
- **invoke** an action: returns the values of the properties it changed.
- **status**: reports `online` or `offline` whenever that changes.

The core promises the driver that calls never overlap and that every value has already been checked against the driver's own descriptors. The driver promises that success means the camera confirmed the change, and that every call ends within its timeout.

### Adding new payload types

- A new MAVLink camera needs only a new profile in the MAVLink driver.
- A new protocol or a new kind of payload needs a new driver. Because the API is made of described properties and actions, a driver for something new, say a laser rangefinder with a read-only `range_m` property and a `measure` action, appears in the API without any change to the core or to the routes.
- Third-party drivers without rebuilding the platform (planned): an external driver is an executable registered for a URI scheme. The platform starts it and speaks the driver contract to it as JSON messages over its standard input and output, so built-in and external drivers look the same to the core.

### Decisions and trade-offs

| Decision | Why | Alternative considered |
| --- | --- | --- |
| HTTP + JSON towards the GCS | works on any IP datalink, any GCS can use it, easy to demonstrate | the MAVLink camera protocol: the drone standard and fine on narrow serial radios, but it has no standard palette message and the GCS would inherit every vendor quirk; gRPC: typed and good at streaming, heavier to demonstrate; MQTT: good for fleets, needs a broker |
| Described properties and actions instead of one route per feature | new payload kinds don't change the API | fixed routes such as `PUT /zoom`: more explicit, but every new feature changes the core |
| Zoom as a magnification factor | physical, and comparable across cameras | percentage of the range, as MAVLink uses |
| Absolute and step zoom on every camera, emulated where missing | the GCS never needs camera-specific logic | expose only what each camera does natively |
| Success means confirmed by the camera | the GCS can trust a 200 | fire and forget: faster, but LC-30T failures would go unnoticed |
| Reads from a cache refreshed every second | quick answers over a slow link | asking the camera on every read |
| Out-of-range numbers refused, not clamped | mistakes stay visible, and the range is in the descriptor | clamp silently, as the LC-30T does |
