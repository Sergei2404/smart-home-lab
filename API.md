# HTTP API

The page served by the lab is a client of this API, so everything it does can be done from scripts
and `curl`. The examples use the default address, `http://127.0.0.1:8765`. The project overview is
in the [README](README.md).

## Endpoints

| Method & path                         | Body                                  | Result            |
|---------------------------------------|---------------------------------------|-------------------|
| `GET /api/devices`                    |                                       | list              |
| `POST /api/devices`                   | `{"name", "kind"}`                    | 201 / 400 / 409 (taken, or the project is full) |
| `PUT /api/devices/{id}/state`         | `{"state": true}` or `{"state": -12}` | 200 / 400 / 404   |
| `PUT /api/devices/{id}/availability`  | `{"online": false}`                   | 200 / 404         |
| `DELETE /api/devices/{id}`            |                                       | 204 / 404 / 409 (still used by a scenario) |
| `GET /api/scenarios`                  |                                       | list              |
| `POST /api/scenarios`                 | `{"name", "repeat"?, "steps"}`        | 201 / 400 / 409 (taken, or the project is full) |
| `PUT /api/scenarios/{id}`             | same                                  | 200 / 400 / 404 / 409 (name taken) |
| `DELETE /api/scenarios/{id}`          |                                       | 204 / 404 (stops a running one) |
| `POST /api/scenarios/{id}/run`        |                                       | 202 / 404 / 409   |
| `POST /api/scenarios/{id}/stop`       |                                       | 204 / 404         |
| `GET /api/runs`                       |                                       | running scenarios |
| `GET /api/events`                     |                                       | SSE stream        |
| `GET /api/log`, `GET /api/status`     |                                       | recent events; MQTT state and active project |
| `GET /api/projects`                   |                                       | `{active, projects, presets}` |
| `POST /api/projects`                  | `{"name", "from"?}`, `from` is `{"preset": id}`, `{"copy_of": id}` or `{"snapshot": {…}}`; empty if left out | 201 / 400 / 404 / 409 / 413 (does not switch) |
| `POST /api/projects/{id}/activate`    |                                       | 200 / 400 (unusable file) / 404 |
| `GET /api/projects/{id}/export`       |                                       | the project file (download) |
| `DELETE /api/projects/{id}`           |                                       | 204 / 404 / 409 (active) |

## Scenarios, ids and limits

A step is `{"type": "set", "device": "lamp", "state": true}` (a number for numeric sensors),
`{"type": "availability", "device": "door", "online": false}` or `{"type": "delay", "secs": 10}`. A state has to fit the device: `true`/`false` for binary devices,
an integer for `percent` and `temperature`, and `percent` is limited to 0–100 — otherwise the
request is rejected with 400, also when it is a step of a scenario.
A scenario needs 1–100 steps, delays of 1–86400 s, and a repeating scenario needs at least one delay.
A project holds at most 500 devices and 100 scenarios (a file or upload above that is refused, and the
UI and API refuse to add more), and a request body may be at most 2 MiB — even a project at all its
limits, exported as readable JSON, fits.
Ids are derived from names (`Living Lamp` → `living-lamp`), so a name needs at least one latin
letter or digit. An id is made once, when the item is created, and **never changes**: renaming a
scenario keeps its id (its URLs keep working) and the old id stays taken, so a new scenario cannot
reuse it (the 409 says which scenario holds the id). A device that a scenario uses cannot be
deleted until the scenario stops using it.

## Errors

Every error is JSON, `{"error": "…"}`: **400** for anything invalid (a malformed body or id, and an
unknown key in a request body, too), **403** for a state-changing request from another origin,
**404**, **405**, **409** for conflicts, **413** for a body over 2 MiB and **415** for a body that
is not `application/json`. Requests that carry an `Origin` header (browsers
do) must have the same origin as the page, so a web page on another site cannot press *Run* for
you; `curl` and other tools send no `Origin` and are not affected.

## Pages that missed a project switch

Devices and scenarios are addressed by id, and two projects may both have a `lamp`. So the UI adds an
`X-Project: <name>` header to every change it sends, naming the project its page shows; if the lab is
running another one (the page missed a switch while its live connection was down) the change is
refused with **409** and the page reloads its state instead of touching the wrong project. Requests
without the header (`curl`, scripts) and everything under `/api/projects` are not affected.

## Examples

```sh
curl -X POST localhost:8765/api/devices -H 'content-type: application/json' \
     -d '{"name": "Living Lamp", "kind": "switch"}'
curl -X POST localhost:8765/api/scenarios -H 'content-type: application/json' \
     -d '{"name": "Night light", "steps": [
           {"type": "set", "device": "living-lamp", "state": true},
           {"type": "delay", "secs": 10},
           {"type": "set", "device": "living-lamp", "state": false}]}'
curl -X POST localhost:8765/api/scenarios/night-light/run

# a numeric sensor: any whole number, negative values included
curl -X POST localhost:8765/api/devices -H 'content-type: application/json' \
     -d '{"name": "Outside", "kind": "temperature"}'
curl -X PUT localhost:8765/api/devices/outside/state -H 'content-type: application/json' \
     -d '{"state": -12}'
```
