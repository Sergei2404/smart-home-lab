'use strict';

const LOG_LIMIT = 200;
const state = {
  devices: [],
  scenarios: [],
  runs: {},
  log: [],
  editor: null,
  mqtt: 'disabled',
  projects: { active: '', projects: [], presets: [] },
  // The project the devices and scenarios on the page belong to. It changes only together with them
  // (in `sync`), unlike `projects.active`, which the project list refreshes on its own.
  shownProject: '',
};
const $ = (id) => document.getElementById(id);

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (key === 'class') node.className = value;
    else if (key in node) node[key] = value;
    else node.setAttribute(key, value);
  }
  node.append(...children);
  return node;
}

async function api(method, path, body) {
  const headers = body === undefined ? {} : { 'Content-Type': 'application/json' };
  // Say which project this page shows: if the lab has moved on (this page missed the switch), the
  // server refuses the change instead of applying it to another project's device of the same name.
  const named = method !== 'GET' && !path.startsWith('/api/projects') && state.shownProject;
  if (named) headers['X-Project'] = state.shownProject;
  const response = await fetch(path, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    const text = await response.text();
    let message = '';
    try {
      message = JSON.parse(text).error;
    } catch {
      // not JSON: fall back to the raw text below
    }
    // A conflict may mean this page is out of date: load the current state (a no-op when it is not).
    if (named && response.status === 409) sync();
    throw new Error(message || text || `HTTP ${response.status}`);
  }
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

async function attempt(action) {
  showError('');
  try {
    await action();
  } catch (error) {
    showError(error.message);
  }
}

function showError(message) {
  const box = $('error');
  box.textContent = message || '';
  box.hidden = !message;
}

const deviceName = (id) => state.devices.find((d) => d.id === id)?.name ?? id;
const scenarioName = (id) => state.scenarios.find((s) => s.id === id)?.name ?? id;
const deviceKind = (id) => state.devices.find((d) => d.id === id)?.kind;

// Numeric kinds hold a whole number; `sample` is what a new scenario starts and ends with.
const KINDS = {
  sensor: { numeric: false },
  switch: { numeric: false },
  relay: { numeric: false },
  percent: { numeric: true, min: 0, max: 100, unit: '%', sample: [100, 0] },
  temperature: { numeric: true, unit: '°C', sample: [25, 0] },
};

function formatValue(kind, value) {
  if (typeof value === 'boolean') return value ? 'ON' : 'OFF';
  const unit = KINDS[kind]?.unit;
  if (!unit) return String(value);
  return unit === '%' ? `${value}%` : `${value} ${unit}`;
}

const sampleValues = (kind) => KINDS[kind]?.sample ?? [true, false];

/** Keeps a step value valid when its device, and so its kind, changes. */
function fitValue(kind, value) {
  const meta = KINDS[kind];
  if (!meta) return value;
  if (!meta.numeric) return typeof value === 'boolean' ? value : false;
  const number = typeof value === 'number' ? value : 0;
  return Math.min(meta.max ?? Infinity, Math.max(meta.min ?? -Infinity, number));
}

const limits = (meta) => ({
  ...(meta?.min !== undefined && { min: meta.min }),
  ...(meta?.max !== undefined && { max: meta.max }),
});

// ---------- devices ----------

function cardHead(device) {
  return [
    el('div', { class: 'card-head' },
      el('strong', {}, device.name),
      el('span', { class: 'badge' }, device.kind)),
    el('div', { class: 'card-meta' },
      el('span', { class: 'card-id' }, device.id),
      availabilityButton(device)),
  ];
}

function availabilityButton(device) {
  const online = device.online !== false;
  return el('button', {
    class: `avail ${online ? 'is-online' : 'is-offline'}`,
    title: online ? 'Simulate the device dropping off the network' : 'Bring the device back online',
    onclick: () => attempt(() => api('PUT', `/api/devices/${device.id}/availability`, { online: !online })),
  }, online ? '● online' : '○ offline');
}

function deleteButton(device) {
  return el('button', {
    class: 'ghost',
    title: `Delete ${device.name}`,
    'aria-label': `Delete ${device.name}`,
    onclick: () => attempt(() => api('DELETE', `/api/devices/${device.id}`)),
  }, '✕');
}

function binaryCard(device) {
  const on = device.state === true;
  const toggle = el('input', {
    type: 'checkbox',
    checked: on,
    'aria-label': `${device.name} state`,
    onchange: (e) => setState(device.id, e.target.checked),
  });
  return el('article', { class: `card ${on ? 'on' : 'off'}${device.online ? '' : ' unavailable'}` },
    ...cardHead(device),
    el('div', { class: 'card-foot' },
      el('span', { class: 'state' }, on ? 'ON' : 'OFF'),
      el('label', { class: 'switch' }, toggle, el('span', { class: 'slider' })),
      deleteButton(device)));
}

function numericCard(device) {
  const meta = KINDS[device.kind];
  const value = el('strong', { class: 'value' }, formatValue(device.kind, device.state));
  const number = el('input', {
    type: 'number',
    step: 1,
    ...limits(meta),
    value: device.state,
    'aria-label': `${device.name} value`,
    onchange: (e) => commitNumber(device, e.target.value),
  });
  const controls = [number];
  if (meta.min !== undefined && meta.max !== undefined) {
    controls.unshift(el('input', {
      type: 'range',
      ...limits(meta),
      value: device.state,
      'aria-label': `${device.name} slider`,
      oninput: (e) => {
        number.value = e.target.value;
        value.textContent = formatValue(device.kind, Number(e.target.value));
      },
      onchange: (e) => commitNumber(device, e.target.value),
    }));
  }
  return el('article', { class: `card numeric${device.online ? '' : ' unavailable'}` },
    ...cardHead(device),
    el('div', { class: 'card-foot value-row' },
      value,
      el('div', { class: 'value-controls' }, ...controls),
      deleteButton(device)));
}

function deviceCard(device) {
  return KINDS[device.kind]?.numeric ? numericCard(device) : binaryCard(device);
}

function commitNumber(device, raw) {
  const value = Number(raw);
  if (raw === '' || !Number.isInteger(value)) {
    showError('Enter a whole number');
    renderDevices();
    return;
  }
  setState(device.id, value);
}

/**
 * Rebuilds only the cards whose data changed and leaves the rest in place, so a slider that is
 * being dragged or a number that is being typed in another card keeps its focus.
 */
function renderCards(container, cache, items, keyOf, build, emptyText) {
  const wanted = new Set(items.map((item) => item.id));
  for (const id of cache.keys()) if (!wanted.has(id)) cache.delete(id);
  const nodes = items.map((item) => {
    const key = keyOf(item);
    const cached = cache.get(item.id);
    if (cached?.key === key) return cached.node;
    const node = build(item);
    cache.set(item.id, { key, node });
    return node;
  });
  if (!nodes.length) nodes.push(el('p', { class: 'empty' }, emptyText));
  nodes.forEach((node, index) => {
    if (container.children[index] !== node) container.insertBefore(node, container.children[index] ?? null);
  });
  while (container.children.length > nodes.length) container.lastElementChild.remove();
}

const deviceCards = new Map();
const scenarioCards = new Map();

function renderDevices() {
  renderCards($('devices'), deviceCards, state.devices, JSON.stringify, deviceCard,
    'No devices yet — add one above.');
}

async function setState(id, value) {
  showError('');
  try {
    await api('PUT', `/api/devices/${id}/state`, { state: value });
  } catch (error) {
    showError(error.message);
    renderDevices();
  }
}

// ---------- scenarios ----------

function describeStep(step) {
  if (step.type === 'delay') return `wait ${step.secs}s`;
  if (step.type === 'availability') {
    return `${deviceName(step.device)} goes ${step.online ? 'online' : 'offline'}`;
  }
  const value = formatValue(deviceKind(step.device), step.state);
  const separator = typeof step.state === 'boolean' ? ' ' : ' = ';
  return `${deviceName(step.device)}${separator}${value}`;
}

function scenarioCard(scenario) {
  const running = scenario.id in state.runs;
  const current = state.runs[scenario.id];
  const steps = scenario.steps.map((step, i) =>
    el('span', { class: `step ${running && i === current ? 'current' : ''}` }, describeStep(step)));
  return el('article', { class: `card scenario ${running ? 'running' : ''}` },
    el('div', { class: 'card-head' },
      el('strong', {}, scenario.name),
      el('span', { class: 'badge' }, scenario.repeat ? 'repeat' : 'once')),
    el('div', { class: 'steps' }, ...steps),
    el('div', { class: 'card-foot' },
      el('span', { class: 'state running-label' },
        running ? `running · step ${current + 1}/${scenario.steps.length}` : ''),
      el('div', { class: 'actions' },
        running
          ? el('button', { onclick: () => attempt(() => api('POST', `/api/scenarios/${scenario.id}/stop`)) }, 'Stop')
          : el('button', { class: 'primary', onclick: () => attempt(() => api('POST', `/api/scenarios/${scenario.id}/run`)) }, 'Run'),
        el('button', { onclick: () => openEditor(scenario) }, 'Edit'),
        el('button', {
          class: 'ghost',
          title: `Delete ${scenario.name}`,
          'aria-label': `Delete ${scenario.name}`,
          onclick: () => attempt(() => api('DELETE', `/api/scenarios/${scenario.id}`)),
        }, '✕'))));
}

function renderScenarios() {
  // Besides the scenario itself, the card shows its run status and the names of its devices.
  const key = (scenario) => JSON.stringify([scenario, state.runs[scenario.id], state.devices.map((d) => d.name)]);
  renderCards($('scenarios'), scenarioCards, state.scenarios, key, scenarioCard, 'No scenarios yet.');
}

// ---------- scenario editor ----------

function starterSteps() {
  const device = state.devices[0];
  if (!device) return [];
  const [start, end] = sampleValues(device.kind);
  return [
    { type: 'set', device: device.id, state: start },
    { type: 'delay', secs: 10 },
    { type: 'set', device: device.id, state: end },
  ];
}

function openEditor(scenario) {
  state.editor = scenario
    ? { id: scenario.id, name: scenario.name, repeat: scenario.repeat, steps: structuredClone(scenario.steps), error: '' }
    : { id: null, name: '', repeat: false, steps: starterSteps(), error: '' };
  renderEditor();
  $('editor').querySelector('input').focus();
}

function closeEditor() {
  state.editor = null;
  renderEditor();
}

function option(value, label, selected) {
  return el('option', { value, selected }, label);
}

function deviceOptions(selected) {
  const options = state.devices.map((d) => option(d.id, d.name, d.id === selected));
  if (selected && !state.devices.some((d) => d.id === selected)) {
    options.unshift(option(selected, `${selected} (missing)`, true));
  }
  return options;
}

function setFields(step) {
  const meta = KINDS[deviceKind(step.device)];
  const numeric = meta ? meta.numeric : typeof step.state === 'number';
  const device = el('select', {
    'aria-label': 'Device',
    onchange: (e) => {
      step.device = e.target.value;
      step.state = fitValue(deviceKind(step.device), step.state);
      renderEditor();
    },
  }, ...deviceOptions(step.device));
  const value = numeric
    ? el('input', {
      type: 'number',
      step: 1,
      required: true,
      ...limits(meta),
      value: step.state,
      'aria-label': 'Value',
      oninput: (e) => { step.state = Number(e.target.value); },
    })
    : el('select', { 'aria-label': 'State', onchange: (e) => { step.state = e.target.value === 'on'; } },
      option('on', 'ON', step.state === true), option('off', 'OFF', step.state !== true));
  return [device, value, ...(meta?.unit ? [el('span', { class: 'unit' }, meta.unit)] : [])];
}

function availabilityFields(step) {
  return [
    el('select', { 'aria-label': 'Device', onchange: (e) => { step.device = e.target.value; } },
      ...deviceOptions(step.device)),
    el('select', { 'aria-label': 'Availability', onchange: (e) => { step.online = e.target.value === 'online'; } },
      option('online', 'goes online', step.online), option('offline', 'goes offline', !step.online)),
  ];
}

const STEP_LABELS = { set: 'Set', availability: 'Status', delay: 'Delay' };

function stepRow(step, index) {
  const steps = state.editor.steps;
  const fields = step.type === 'set'
    ? setFields(step)
    : step.type === 'availability'
      ? availabilityFields(step)
      : [
      el('span', {}, 'wait'),
      el('input', {
        type: 'number', min: 1, max: 86400, required: true, value: step.secs, 'aria-label': 'Seconds',
        oninput: (e) => { step.secs = Number(e.target.value); },
      }),
      el('span', {}, 'seconds'),
    ];
  const move = (by) => () => {
    steps.splice(index + by, 0, ...steps.splice(index, 1));
    renderEditor();
  };
  return el('li', { class: 'step-row' },
    el('span', { class: 'step-kind' }, STEP_LABELS[step.type] ?? step.type),
    ...fields,
    el('span', { class: 'step-actions' },
      el('button', { type: 'button', class: 'ghost', title: 'Move up', disabled: index === 0, onclick: move(-1) }, '↑'),
      el('button', { type: 'button', class: 'ghost', title: 'Move down', disabled: index === steps.length - 1, onclick: move(1) }, '↓'),
      el('button', {
        type: 'button', class: 'ghost', title: 'Remove step',
        onclick: () => { steps.splice(index, 1); renderEditor(); },
      }, '✕')));
}

function renderEditor() {
  const editor = state.editor;
  const box = $('editor');
  box.hidden = !editor;
  if (!editor) {
    box.replaceChildren();
    return;
  }
  const addStep = (makeStep) => () => {
    editor.steps.push(makeStep());
    renderEditor();
  };
  box.replaceChildren(el('form', { class: 'editor', onsubmit: (e) => { e.preventDefault(); saveEditor(); } },
    el('div', { class: 'row' },
      el('input', {
        placeholder: 'Scenario name', maxLength: 64, required: true, value: editor.name, 'aria-label': 'Scenario name',
        oninput: (e) => { editor.name = e.target.value; },
      }),
      el('label', { class: 'check' },
        el('input', { type: 'checkbox', checked: editor.repeat, onchange: (e) => { editor.repeat = e.target.checked; } }),
        ' repeat until stopped')),
    el('ol', { class: 'step-list' }, ...editor.steps.map(stepRow)),
    el('div', { class: 'row' },
      el('button', {
        type: 'button', disabled: state.devices.length === 0,
        title: state.devices.length ? '' : 'Add a device first',
        onclick: addStep(() => {
          const device = state.devices[0];
          return { type: 'set', device: device?.id ?? '', state: sampleValues(device?.kind)[0] };
        }),
      }, '+ Set device'),
      el('button', {
        type: 'button', disabled: state.devices.length === 0,
        title: state.devices.length ? 'Take a device offline or bring it back' : 'Add a device first',
        onclick: addStep(() => ({ type: 'availability', device: state.devices[0]?.id ?? '', online: false })),
      }, '+ Availability'),
      el('button', { type: 'button', onclick: addStep(() => ({ type: 'delay', secs: 10 })) }, '+ Delay')),
    ...(editor.error ? [el('p', { class: 'error' }, editor.error)] : []),
    el('div', { class: 'row' },
      el('button', { type: 'submit', class: 'primary' }, editor.id ? 'Save' : 'Create'),
      el('button', { type: 'button', onclick: closeEditor }, 'Cancel'))));
}

async function saveEditor() {
  const editor = state.editor;
  const body = { name: editor.name, repeat: editor.repeat, steps: editor.steps };
  try {
    if (editor.id) await api('PUT', `/api/scenarios/${editor.id}`, body);
    else await api('POST', '/api/scenarios', body);
    closeEditor();
  } catch (error) {
    editor.error = error.message;
    renderEditor();
  }
}

// ---------- log ----------

const sourceLabel = (source) =>
  source.type === 'scenario' ? `scenario: ${scenarioName(source.id)}` : source.type;

function describeEvent(event) {
  switch (event.type) {
    case 'device_added':
      return el('span', {}, `+ ${event.device.name} added (${event.device.kind})`);
    case 'device_removed':
      return el('span', {}, `− ${deviceName(event.id)} removed`);
    case 'state_changed': {
      const shade = typeof event.state === 'boolean' ? (event.state ? 'on' : 'off') : 'num';
      return el('span', {},
        `${deviceName(event.id)} → `,
        el('b', { class: shade }, formatValue(deviceKind(event.id), event.state)),
        el('span', { class: 'src' }, `  (${sourceLabel(event.source)})`));
    }
    case 'availability_changed':
      return el('span', {},
        `${deviceName(event.id)} → `,
        el('b', { class: event.online ? 'on' : 'off' }, event.online ? 'online' : 'offline'),
        el('span', { class: 'src' }, `  (${sourceLabel(event.source)})`));
    case 'scenario_added':
      return el('span', {}, `+ scenario ${event.scenario.name} added`);
    case 'scenario_updated':
      return el('span', {}, `✎ scenario ${event.scenario.name} updated`);
    case 'scenario_removed':
      return el('span', {}, `− scenario ${scenarioName(event.id)} removed`);
    case 'scenario_started':
      return el('span', {}, `▶ ${scenarioName(event.id)} started`);
    case 'scenario_finished':
      return el('span', {}, `■ ${scenarioName(event.id)} finished`);
    case 'scenario_stopped':
      return el('span', {}, `■ ${scenarioName(event.id)} stopped`);
    case 'scenario_failed':
      return el('span', { class: 'failed' }, `✕ ${scenarioName(event.id)} failed: ${event.reason}`);
    case 'mqtt_status':
      return el('span', {}, `MQTT ${event.state}`);
    case 'project_changed':
      return el('span', {}, `⇄ switched to project ${event.project}`);
    default:
      return el('span', {}, event.type);
  }
}

function renderLog() {
  const items = [...state.log].reverse().map((event) =>
    el('li', {},
      el('time', { dateTime: event.at }, new Date(event.at).toLocaleTimeString()),
      describeEvent(event)));
  $('log').replaceChildren(...items);
}

// ---------- live updates ----------

function renderMqtt() {
  const pill = $('mqtt');
  const labels = { disabled: 'off', disconnected: 'disconnected', connected: 'connected' };
  const classes = { disabled: 'pill-off', disconnected: 'pill-warn', connected: 'pill-on' };
  pill.textContent = `MQTT: ${labels[state.mqtt] ?? state.mqtt}`;
  pill.className = `pill ${classes[state.mqtt] ?? 'pill-off'}`;
}

// ---------- projects ----------

const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;

function setProjectsError(message) {
  const box = $('projects-error');
  box.textContent = message || '';
  box.hidden = !message;
  // The dialog scrolls; the box sits at its top, so bring it into view when something went wrong.
  if (message) box.scrollIntoView({ block: 'nearest' });
}

/** Puts the header picker back on the active project (after a cancelled or failed switch). */
function resetPicker() {
  $('project').value = state.projects.active;
}

/** Runs a project action; a failure is shown in the dialog if it is open, else in the page banner. */
async function projectAction(action) {
  const dialogOpen = $('projects-dialog').open;
  setProjectsError('');
  showError('');
  try {
    await action();
  } catch (error) {
    if (dialogOpen) setProjectsError(error.message);
    else showError(error.message);
    resetPicker();
  }
}

async function switchProject(id) {
  if (id === state.projects.active) return;
  const running = Object.keys(state.runs).length;
  if (running && !window.confirm(
    `${plural(running, 'scenario')} running in "${state.projects.active}" will be stopped. Switch to "${id}"?`)) {
    resetPicker();
    return;
  }
  await api('POST', `/api/projects/${id}/activate`);
}

const MAX_ID_LENGTH = 32;

/** A free name for a copy of `id`: `<id>-copy`, `<id>-copy-2`, … cut so that it still fits an id. */
function copyName(id) {
  const taken = new Set(state.projects.projects.map((p) => p.id));
  for (let n = 1; ; n += 1) {
    const suffix = n === 1 ? '-copy' : `-copy-${n}`;
    const candidate = id.slice(0, MAX_ID_LENGTH - suffix.length).replace(/[-_]+$/, '') + suffix;
    if (!taken.has(candidate)) return candidate;
  }
}

function projectRow(project) {
  const active = project.id === state.projects.active;
  const meta = project.error
    ? `unusable: ${project.error}`
    : `${plural(project.devices, 'device')} · ${plural(project.scenarios, 'scenario')}`;
  return el('li', { class: `project-row${active ? ' active' : ''}` },
    el('span', { class: 'project-name' }, project.id),
    ...(active ? [el('span', { class: 'tag' }, 'active')] : []),
    el('span', { class: 'project-meta', title: meta }, meta),
    el('span', { class: 'project-actions' },
      el('button', {
        type: 'button',
        disabled: active || Boolean(project.error),
        onclick: () => projectAction(() => switchProject(project.id)),
      }, 'Activate'),
      el('button', {
        type: 'button',
        disabled: Boolean(project.error),
        onclick: () => projectAction(async () => {
          const name = window.prompt('Name for the copy', copyName(project.id));
          if (name) await api('POST', '/api/projects', { name, from: { copy_of: project.id } });
        }),
      }, 'Duplicate'),
      project.error
        ? el('span', { class: 'btn-link disabled', 'aria-disabled': 'true' }, 'Export')
        : el('a', {
          class: 'btn-link',
          href: `/api/projects/${project.id}/export`,
          download: `${project.id}.json`,
        }, 'Export'),
      el('button', {
        type: 'button',
        class: 'ghost',
        title: active ? 'Switch to another project to delete this one' : `Delete ${project.id}`,
        'aria-label': `Delete ${project.id}`,
        disabled: active,
        onclick: () => projectAction(async () => {
          if (window.confirm(`Delete project "${project.id}" for good?`)) {
            await api('DELETE', `/api/projects/${project.id}`);
          }
        }),
      }, '✕')));
}

// What the picker and the dialog currently show. They are rebuilt only when it changes: this runs
// for every sync, and rebuilding a list under the user's hands loses focus and closes open menus.
let shownProjects = null;

function renderProjects() {
  const key = JSON.stringify(state.projects);
  if (key === shownProjects) return;
  shownProjects = key;
  const { active, projects, presets } = state.projects;

  const picker = $('project');
  picker.replaceChildren(...projects.map((p) => option(p.id, p.error ? `${p.id} (unusable)` : p.id, p.id === active)));
  picker.value = active;

  $('project-list').replaceChildren(...projects.map(projectRow));

  const source = document.querySelector('#new-project select[name="source"]');
  const chosen = source.value;
  source.replaceChildren(
    option('empty', 'Empty project', true),
    option('copy', `Copy of ${active || 'the current project'}`, false),
    ...presets.map((p) => option(`preset:${p.id}`, `${p.title}: ${plural(p.devices, 'device')}, ${plural(p.scenarios, 'scenario')}`, false)),
  );
  if ([...source.options].some((o) => o.value === chosen)) source.value = chosen;
}

async function refreshProjects() {
  try {
    state.projects = await api('GET', '/api/projects');
    renderProjects();
  } catch (error) {
    showError(error.message);
  }
}

function render() {
  renderMqtt();
  renderDevices();
  renderScenarios();
  renderLog();
}

function onEvent(event) {
  if (event.type === 'scenario_step') {
    state.runs[event.id] = event.step;
    renderScenarios();
    return;
  }
  if (event.type === 'projects_changed') {
    refreshProjects();
    return;
  }
  if (event.type === 'project_changed') {
    // Everything on the page belongs to the old project: drop it and load the new one.
    state.editor = null;
    renderEditor();
    sync();
    return;
  }
  switch (event.type) {
    case 'device_added':
      if (!state.devices.some((d) => d.id === event.device.id)) state.devices.push(event.device);
      break;
    case 'device_removed':
      state.devices = state.devices.filter((d) => d.id !== event.id);
      break;
    case 'state_changed': {
      const device = state.devices.find((d) => d.id === event.id);
      if (device) device.state = event.state;
      break;
    }
    case 'availability_changed': {
      const device = state.devices.find((d) => d.id === event.id);
      if (device) device.online = event.online;
      break;
    }
    case 'scenario_added':
      if (!state.scenarios.some((s) => s.id === event.scenario.id)) state.scenarios.push(event.scenario);
      break;
    case 'scenario_updated': {
      const index = state.scenarios.findIndex((s) => s.id === event.scenario.id);
      if (index >= 0) state.scenarios[index] = event.scenario;
      break;
    }
    case 'scenario_removed':
      state.scenarios = state.scenarios.filter((s) => s.id !== event.id);
      delete state.runs[event.id];
      break;
    case 'scenario_started':
      state.runs[event.id] = 0;
      break;
    case 'mqtt_status':
      state.mqtt = event.state;
      break;
    case 'scenario_finished':
    case 'scenario_stopped':
    case 'scenario_failed':
      delete state.runs[event.id];
      break;
  }
  state.log.push(event);
  if (state.log.length > LOG_LIMIT) state.log.shift();
  render();
}

let syncing = false;
let changedWhileSyncing = false;

/**
 * Loads the full state. The five requests are not one atomic snapshot, and an event that arrives
 * while they are in flight could be overwritten by older data, so if anything happened meanwhile
 * the state is fetched again (a few times at most).
 */
async function sync() {
  if (syncing) {
    changedWhileSyncing = true;
    return;
  }
  syncing = true;
  try {
    for (let attempt = 0; attempt < 3; attempt += 1) {
      changedWhileSyncing = false;
      const [devices, scenarios, runs, log, status, projects] = await Promise.all([
        api('GET', '/api/devices'),
        api('GET', '/api/scenarios'),
        api('GET', '/api/runs'),
        api('GET', '/api/log'),
        api('GET', '/api/status'),
        api('GET', '/api/projects'),
      ]);
      Object.assign(state, {
        devices,
        scenarios,
        log,
        mqtt: status.mqtt,
        projects,
        shownProject: projects.active,
        runs: Object.fromEntries(runs.map((r) => [r.id, r.step])),
      });
      renderProjects();
      render();
      if (!changedWhileSyncing) break;
    }
  } catch (error) {
    showError(error.message);
  } finally {
    syncing = false;
  }
}

function setConnected(connected) {
  const pill = $('conn');
  pill.textContent = connected ? 'live' : 'reconnecting…';
  pill.className = `pill ${connected ? 'pill-on' : 'pill-off'}`;
}

function connect() {
  const source = new EventSource('/api/events');
  source.onopen = () => {
    setConnected(true);
    sync();
  };
  source.onerror = () => setConnected(false);
  source.onmessage = (message) => {
    if (syncing) changedWhileSyncing = true;
    onEvent(JSON.parse(message.data));
  };
  source.addEventListener('resync', sync);
}

$('add-device').addEventListener('submit', (e) => {
  e.preventDefault();
  const form = e.currentTarget;
  attempt(async () => {
    await api('POST', '/api/devices', {
      name: form.elements.name.value,
      kind: form.elements.kind.value,
    });
    form.elements.name.value = '';
    form.elements.name.focus();
  });
});

$('new-scenario').addEventListener('click', () => openEditor(null));

$('project').addEventListener('change', (e) => {
  const id = e.target.value;
  projectAction(() => switchProject(id));
});

$('manage-projects').addEventListener('click', () => {
  setProjectsError('');
  $('projects-dialog').showModal();
});

$('new-project').addEventListener('submit', (e) => {
  e.preventDefault();
  const form = e.currentTarget;
  const [kind, argument] = form.elements.source.value.split(':');
  const body = { name: form.elements.name.value };
  if (kind === 'preset') body.from = { preset: argument };
  if (kind === 'copy') body.from = { copy_of: state.projects.active };
  projectAction(async () => {
    const created = await api('POST', '/api/projects', body);
    if (form.elements.switch.checked) await switchProject(created.id);
    form.reset();
  });
});

// A file's name is a sensible default name for the project made from it.
$('import-project').elements.file.addEventListener('change', (e) => {
  const { form, files } = e.currentTarget;
  if (files[0] && !form.elements.name.value.trim()) {
    form.elements.name.value = files[0].name.replace(/\.json$/i, '');
  }
});

$('import-project').addEventListener('submit', (e) => {
  e.preventDefault();
  const form = e.currentTarget;
  projectAction(async () => {
    const text = await form.elements.file.files[0].text();
    let snapshot;
    try {
      snapshot = JSON.parse(text);
    } catch {
      throw new Error('The file is not valid JSON');
    }
    await api('POST', '/api/projects', { name: form.elements.name.value, from: { snapshot } });
    form.reset();
  });
});

renderProjects();
render();
connect();
