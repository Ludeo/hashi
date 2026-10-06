const OFFICIAL_ORIGIN = "https://mania-tracker.com";
const BRIDGE = `http://127.0.0.1:${new URLSearchParams(location.search).get("bridge") || 24071}`;
const app = document.getElementById("app");

let snapshot = null;
let helperDown = false;
let busy = false;
// controls only dim when a save takes a while, a quick one would just flicker them
let slow = false;
let unsaved = null;

// kind: spin, done, wait, fail, off
function playStatus(p) {
  const r = p.receipt;
  const retryAt = p.retryAt && `Trying again at ${time(new Date(p.retryAt).toISOString())}`;
  if (p.onHold) return { kind: "wait", label: "On hold", detail: "Not sent until you send it." };
  if (!snapshot.sendPlays && p.phase === "send") return { kind: "wait", label: "Not sending", detail: "Sending plays is off." };
  if (p.phase === "waiting_replay") return { kind: "spin", label: "Waiting for replay" };
  if (p.phase === "unbound") return { kind: "off", label: "Not sent", detail: "Wasn't connected when you played" };
  if (p.phase === "held") return { kind: "wait", label: "On hold", detail: p.error?.message };
  if (p.phase === "failed") return { kind: "fail", label: "Not sent", detail: p.error?.message };
  if (p.error?.code === "retrying") return { kind: "wait", label: "Retrying", detail: retryAt };
  if (p.error?.code === "gone") return { kind: "fail", label: "Not submitted", detail: p.error.message };
  switch (r?.state) {
    case undefined:
    case "awaiting_assets": return { kind: "spin", label: "Uploading" };
    case "queued": return { kind: "spin", label: "In queue" };
    case "validating":
    case "analyzing": return { kind: "spin", label: "Processing" };
    case "deferred": return { kind: "spin", label: "Processing", detail: "Taking a little longer than usual" };
    case "accepted": return { kind: "done", label: "Submitted" };
    case "rejected":
      if (r.error?.code === "duplicate_play") return { kind: "done", label: "Already submitted" };
      return { kind: "fail", label: "Not submitted", detail: r.error?.message };
    case "expired": return { kind: "fail", label: "Not submitted", detail: "The upload took too long" };
    case "deleted": return { kind: "off", label: "Removed" };
    default: return { kind: "spin", label: "Processing" };
  }
}

const SPIN_MS = 900;
const covers = new Map();

// only stamp plays that change while on screen, not everything at startup
const stamped = new Map();
const stampAt = new Map();

function stamp(status, id) {
  const before = stamped.get(id);
  stamped.set(id, status.label);
  if (status.kind === "spin") {
    return el("span", { class: "doing" },
      el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }), status.label);
  }
  if (before !== undefined && before !== status.label) stampAt.set(id, Date.now());
  const tilt = -2 - [...id].reduce((sum, ch) => sum + ch.charCodeAt(0), 0) % 6;
  return el("span", {
    class: `stamp ${status.kind}`,
    style: `--tilt: ${tilt}deg; animation-delay: -${Date.now() - (stampAt.get(id) ?? 0)}ms`,
  }, status.label);
}

// icons and colors from the site's ModBadge
const MOD_FILES = {
  HD: "hidden", HR: "hard-rock", DT: "double-time", FL: "flashlight", EZ: "easy", NF: "no-fail",
  HT: "half-time", NC: "nightcore", SD: "sudden-death", PF: "perfect", FI: "fade-in", RD: "random",
  MR: "mirror", SV2: "score-v2", TD: "touch-device", CN: "cinema", CO: "cover", DS: "dual-stages",
  "1K": "one-key", "2K": "two-keys", "3K": "three-keys", "4K": "four-keys", "5K": "five-keys",
  "6K": "six-keys", "7K": "seven-keys", "8K": "eight-keys", "9K": "nine-keys", "10K": "ten-keys",
};
const MOD_COLORS = {
  red: ["HD", "HR", "DT", "FL", "NC", "SD", "PF", "FI", "CO"],
  lime: ["EZ", "NF", "HT"],
  purple: ["MR", "RD", "DS", "1K", "2K", "3K", "4K", "5K", "6K", "7K", "8K", "9K", "10K"],
  blue: ["CN", "AT", "RX", "AP"],
  yellow: ["TD", "SV2"],
};
const COLOR = { red: "#ff6666", lime: "#b2ff66", purple: "#8c66ff", blue: "#66ccff", yellow: "#ffcc22" };
const modColor = (mod) => COLOR[Object.keys(MOD_COLORS).find((c) => MOD_COLORS[c].includes(mod))] ?? COLOR.red;

// tosu gives "HDNC", "4KMR" etc
function modList(name) {
  const mods = name?.match(/10K|[1-9]K|SV2|[A-Z]{2}/g) ?? [];
  return mods.filter((m) => !(m === "DT" && mods.includes("NC")) && !(m === "SD" && mods.includes("PF")));
}

function modBadge(mod) {
  const file = MOD_FILES[mod];
  return el("span", { class: "mod", title: mod, style: `--mod: ${modColor(mod)}` },
    file
      ? el("i", { style: `-webkit-mask-image: url("images/mods/mod-${file}.svg"); mask-image: url("images/mods/mod-${file}.svg")` })
      : el("b", {}, mod));
}

function cover(p) {
  let node = covers.get(p.id);
  if (!node) {
    node = el("div", { class: "cover" });
    if (p.cover) {
      const img = new Image();
      img.onload = () => {
        node.style.setProperty("--cover", `url("${img.src}")`);
        node.classList.add("loaded");
      };
      img.src = `${BRIDGE}/cover/${encodeURIComponent(p.id)}`;
    }
    covers.set(p.id, node);
  }
  return node;
}

const OSU_STATES = {
  menu: "Main menu",
  selectPlay: "Song select",
  play: "Playing",
  resultScreen: "Results",
  edit: "Editor",
  selectEdit: "Editor",
  selectMulti: "Multiplayer",
  lobby: "Multiplayer",
  matchSetup: "Multiplayer",
  selectDrawings: "Song select",
  onlineSelection: "osu!direct",
};

const TOSU_HINT = "Start tosu to submit plays. Anything you play without it isn't sent.";

function gameName(tosu) {
  return tosu.client === "lazer" ? "osu!lazer" : "osu!";
}

function osuLabel(tosu) {
  return OSU_STATES[tosu.state] ?? String(tosu.state ?? "").replace(/([a-z])([A-Z])/g, "$1 $2").replace(/^./, (c) => c.toUpperCase());
}

function tosuDetail(tosu) {
  return { starting: "Starting", failed: "Couldn't start", missing: "Not found", "with-osu": "Opens with osu!" }[tosu.launch] ?? "Not running";
}

function blockedHint() {
  const { blocked } = snapshot.tosu;
  return blocked && el("div", { class: "blocked" },
    el("p", {}, "Linux blocks tosu from reading the game. Run this once, then restart Mania Bridge:"),
    el("code", { title: "Click to copy", onclick: (e) => {
      navigator.clipboard?.writeText(blocked.fix);
      e.currentTarget.classList.add("copied");
    } }, blocked.fix));
}

function getTosu() {
  const g = snapshot.getTosu;
  if (!g) return null;
  const get = () => action("get-tosu");
  if (g.step === "downloading" || g.step === "unpacking") {
    return el("div", { class: "pinned" },
      el("p", {}, g.step === "unpacking"
        ? "Unpacking tosu…"
        : ["Downloading tosu", g.percent != null ? ` · ${g.percent}%` : "", "…"]),
      el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }));
  }
  if (g.step === "failed") {
    return el("div", { class: "pinned failed" },
      el("p", {}, "Couldn't get tosu. ", el("span", { class: "why" }, g.error)),
      el("button", { class: "key go", disabled: slow, onclick: get }, "Try again"));
  }
  return el("div", { class: "pinned" },
    el("p", {}, "Couldn't find tosu on this computer. Mania Bridge can get it for you."),
    el("button", { class: "key go", disabled: slow, onclick: get }, "Get tosu"));
}

function tosuHint() {
  const { tosu } = snapshot;
  if (tosu.blocked) return blockedHint();
  if (snapshot.getTosu) return getTosu();
  if (tosu.connected || tosu.launch === "starting" || tosu.launch === "with-osu") return null;
  return el("p", { class: "status warn" }, TOSU_HINT);
}

// loops are offset from the clock so a re-render doesn't restart them
function loop(ms, at = 0) {
  return `animation-duration: ${ms}ms; animation-delay: -${(Date.now() + at * ms) % ms}ms`;
}

const LEAF_MS = 700;
const BOB_MS = 2400;
let leafPose = "up";
let leafAt = 0;

function leaf(pose) {
  if (pose !== leafPose) {
    leafPose = pose;
    leafAt = Date.now();
  }
  const moved = Math.min(Date.now() - leafAt, LEAF_MS);
  const bob = moved < LEAF_MS ? LEAF_MS - moved : -((Date.now() - leafAt - LEAF_MS) % BOB_MS);
  return `class="leaf ${pose}" style="animation-delay: -${moved}ms, ${bob}ms"`;
}

const STILL = matchMedia("(prefers-reduced-motion: reduce)");
const CROSS_MS = 1350;
let crossNoteMs = 1200;
let reach = 300;
let wasConnecting = false;
let crossAt = 0;
let crossTimer = null;

function cross() {
  crossAt = Date.now();
  clearTimeout(crossTimer);
  crossTimer = setTimeout(iris, CROSS_MS);
}

// plain fills under a clip-path, an ink filter over the whole window is too slow to animate
async function iris() {
  const sign = document.querySelector(".sign.landing circle")?.getBoundingClientRect();
  if (!crossAt || !sign) return;
  const from = { x: sign.x + sign.width / 2, y: sign.y + sign.height / 2, r: sign.width / 2 };
  const cover = el("div", { class: "iris" }, el("i"), el("i"));
  document.body.append(cover);
  const move = (a, b, timing) => Promise.all([...cover.children].map((layer, i) => {
    const clip = ({ x, y, r }) => `circle(${Math.max(0, r + (i ? -1.5 : 1.5))}px at ${x}px ${y}px)`;
    return layer.animate({ clipPath: [clip(a), clip(b)] }, { ...timing, fill: "forwards" }).finished;
  }));
  const full = { ...from, r: Math.hypot(Math.max(from.x, innerWidth - from.x), Math.max(from.y, innerHeight - from.y)) + 8 };
  await move(from, full, { duration: 380, easing: "cubic-bezier(0.5, 0, 0.9, 0.4)" });
  crossAt = 0;
  render();
  const face = document.querySelector(".me .face")?.getBoundingClientRect();
  const to = face ? { x: face.x + face.width / 2, y: face.y + face.height / 2, r: face.width / 2 } : { ...from, r: 0 };
  await move(full, to, { duration: 420, easing: "cubic-bezier(0.3, 0.6, 0.2, 1)" });
  await cover.animate({ opacity: [1, 0] }, { duration: 160, fill: "forwards" }).finished;
  cover.remove();
}

const NOTE_MS = 3600;

function bridge(rows, pose) {
  const crossed = Date.now() - crossAt;
  const landing = pose === "flat" && crossAt;
  const notes = pose !== "flat" && rows.osu.state === "ok" ? [0, 1, 2].map((i) => loop(NOTE_MS, i / 3)) : [];
  const overs = pose === "flat"
    ? [0, 1, 2].map((i) => `animation-duration: ${crossNoteMs}ms; animation-delay: ${150 + i * crossNoteMs / 3 - crossed}ms`)
    : [];
  // first note reaches the sign 72% into its loop
  const lit = 150 + crossNoteMs * 0.72;
  const water = `M${-reach} 246q10-3 20 0${"t20 0".repeat(reach / 10 + 19)}V300H${-reach}z`;
  // filled outside the wobble: webkitgtk's filters shift dark colors a little, and the river under it isn't filtered
  const drift = loop(4000);
  const node = el("div", { class: "scene" }, Object.entries(rows).map(([at, r]) =>
    el("div", { class: `tag at-${at} is-${r.state}` },
      el("b", {}, r.name),
      r.detail && el("span", { class: "detail" }, r.detail),
      r.sub && el("span", { class: "sub" }, r.sub))));
  node.insertAdjacentHTML("afterbegin", `<svg viewBox="0 44 400 224" aria-hidden="true"><g class="ink">
    <path class="land" d="M${-reach} 200H54q10 1 12 14l8 60H${-reach}z"/>
    <path class="land" d="M${400 + reach} 200H346q-10 1-12 14l-8 60H${400 + reach}z"/>
    <path class="line" d="M69 233q40-31 83-7M331 233q-40-31-83-7"/>
    <rect class="solid" x="152" y="205" width="16" height="70"/>
    <rect class="solid" x="232" y="205" width="16" height="70"/>
    <g ${leaf(pose)}><rect class="solid" x="166" y="199" width="72" height="8" rx="2"/><path class="line" d="M184 199v8M202 199v8M220 199v8"/></g>
    <rect class="solid" x="46" y="200" width="122" height="7" rx="2"/>
    <rect class="solid" x="232" y="200" width="122" height="7" rx="2"/>
    <path class="solid" d="M229 200v-25l11-10 11 10v25z"/>
    <path class="line" d="M240 181v8"/>
    <g class="light is-${rows.tosu.state}">
      <path class="line" d="M130 200v-31"/>
      <rect class="solid" x="121" y="134" width="18" height="36" rx="8"/>
      <circle class="lamp stop" cx="130" cy="145" r="4.5" style="${loop(1000)}"/>
      <circle class="lamp go" cx="130" cy="159" r="4.5"/>
    </g>
    ${notes.map((style) => `<g class="note" style="${style}"><rect x="-10" y="-9" width="20" height="9" rx="4.5"/></g>`).join("")}
    ${overs.map((style) => `<g class="note over" style="${style}"><rect x="-10" y="-9" width="20" height="9" rx="4.5"/></g>`).join("")}
    <g class="sign is-${rows.osu.state}">
      <path class="line" d="M30 186v14"/>
      <circle cx="30" cy="164" r="22"/>
      <text x="30" y="168.5" text-anchor="middle">osu!</text>
    </g>
    <g class="sign is-${rows.site.state}${landing ? " landing" : ""}" style="--lit: ${lit}ms; --waited: -${crossed}ms">
      <path class="line" d="M370 186v14"/>
      <circle cx="370" cy="164" r="22"/>
      <path class="glyph" d="M356 159h12v-8l14 13-14 13v-8h-12z" style="${pose === "flat" ? "" : loop(1000)}"/>
    </g>
    ${landing ? `<path class="line spark" d="M-14-4l-7-8M0-9v-11M14-4l7-8" style="animation-delay: ${lit - crossed}ms"/>` : ""}
  </g>
    <path class="water" d="${water}" style="${drift}"/>
    <g class="ink">
    <path class="water edge" d="${water}" style="${drift}"/>
    <path class="ripple" d="M88 259q5-3 10 0t10 0M286 262q5-3 10 0t10 0M12 263q5-3 10 0"/>
    ${notes.map((style) => `<path class="line splash" d="M-8-2l-5-7M0-4v-9M8-2l5-7" style="${style}"/>`).join("")}
  </g></svg>`);
  return node;
}

function riverbed() {
  const node = el("div", { class: "bed" });
  node.innerHTML = `<svg viewBox="0 0 400 76" aria-hidden="true"><g class="ink">
    <g class="fish" style="--from: ${-reach + 250}px; --to: ${reach + 150}px; ${loop(44 * (reach * 2 - 100))}"><path d="M-10 0q9-9 19 0q-10 9-19 0zM-10 0l-7-5v10z"/><circle cx="4" cy="-1" r="1"/></g>
    <path class="line" d="M${-reach} 66q25-5 50 0${"t50 0".repeat(reach / 25 + 7)}"/>
    <path class="line" d="M322 65q-5-8 0-15t0-14M333 65q4-7 0-12t1-11M58 66q-4-6 0-11t-1-9"/>
    <rect class="sunk" x="172" y="55" width="20" height="9" rx="4.5" transform="rotate(-9 182 60)"/>
    <rect class="sunk" x="196" y="56" width="20" height="9" rx="4.5" transform="rotate(6 206 60)"/>
    <rect class="sunk" x="183" y="45" width="20" height="9" rx="4.5" transform="rotate(-24 193 50)"/>
  </g></svg>`;
  return node;
}

function shore() {
  const node = el("div", { class: "shore" });
  const width = Math.max(innerWidth, 400);
  // as wide as the window could ever get
  const water = Math.max(screen.width, width);
  node.innerHTML = `<svg width="100%" height="72" aria-hidden="true"><g class="still">
    <svg y="100%" overflow="visible">
      <path class="water" d="M-40 -66q10-3 20 0${"t20 0".repeat(Math.ceil(water / 20) + 6)}V10H-40z" style="${loop(9000)}"/>
      <g transform="translate(0 -56)"><g class="fish" style="--from: -40px; --to: ${width + 40}px; ${loop(88 * (width + 80))}"><path d="M-10 0q9-9 19 0q-10 9-19 0zM-10 0l-7-5v10z"/><circle cx="4" cy="-1" r="1"/></g></g>
      <path class="line" d="M28 0q-5-10 0-19t0-17t1-12M40 0q4-9 0-15t1-13M52 0q-3-6 0-11"/>
      <svg x="64%" overflow="visible">
        <rect class="sunk" x="-10" y="-14" width="20" height="9" rx="4.5" transform="rotate(-9 0 -10)"/>
        <rect class="sunk" x="14" y="-13" width="20" height="9" rx="4.5" transform="rotate(6 24 -9)"/>
        <rect class="sunk" x="2" y="-24" width="20" height="9" rx="4.5" transform="rotate(-24 12 -20)"/>
      </svg>
      <svg x="100%" overflow="visible">
        <path class="line" d="M-32 0q-4-9 0-16t0-14t1-10M-44 0q4-7 0-12t1-10"/>
        <rect class="sunk" x="-96" y="-15" width="20" height="9" rx="4.5" transform="rotate(-14 -86 -11)"/>
      </svg>
    </svg>
  </g></svg>`;
  return node;
}

function rows(site) {
  const { tosu } = snapshot;
  const osuOpen = tosu.connected && tosu.osu;
  return {
    osu: osuOpen
      ? { state: "ok", name: gameName(tosu), detail: osuLabel(tosu), sub: tosu.player || "Guest" }
      : tosu.blocked
        ? { state: "warn", name: "osu!", detail: "Can't be read" }
        : { state: "idle", name: "osu!", detail: "Not open" },
    tosu: tosu.connected
      ? { state: "ok", name: "tosu", detail: "Running" }
      : { state: { starting: "spin", "with-osu": "idle" }[tosu.launch] ?? "warn", name: "tosu", detail: tosuDetail(tosu) },
    site,
  };
}

function intro(pose) {
  return bridge(rows({
    up: { state: "idle", name: "Mania Tracker", detail: "Not connected" },
    down: { state: "spin", name: "Mania Tracker", detail: "Waiting for you" },
    flat: { state: "ok", name: "Mania Tracker", detail: "Connected" },
  }[pose]), pose);
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (k === "class") node.className = v;
    else if (k === "style") node.style.cssText = v;
    else node.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) {
    if (c == null || c === false) continue;
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

// 12/24h from the system locale
function time(iso) {
  const clock = snapshot.clock ?? {};
  let hour12 = clock.hour12;
  if (typeof hour12 !== "boolean") {
    try { hour12 = new Intl.DateTimeFormat(clock.locale ?? undefined, { hour: "numeric" }).resolvedOptions().hour12; } catch {}
  }
  return new Date(iso).toLocaleTimeString("en-US", { hour: hour12 ? "numeric" : "2-digit", minute: "2-digit", hour12: !!hour12 });
}

async function action(name) {
  if (busy) return;
  busy = true;
  unsaved = null;
  lockedHint = null;
  const dim = setTimeout(() => {
    slow = true;
    render();
  }, 300);
  try {
    const res = await fetch(`${BRIDGE}/${name}`, { method: "POST" });
    if (res.ok) snapshot = await res.json();
    else if (res.status === 500) unsaved = (await res.json().catch(() => null))?.error ?? "Couldn't save that.";
  } catch {
    helperDown = true;
  } finally {
    clearTimeout(dim);
    busy = slow = false;
    render();
  }
}

function chartName(play) {
  const keys = play.keys ? `${Math.round(play.keys)}K ` : "";
  const version = play.version ?? "";
  return `${play.title ?? "Unknown"} [${keys}${version}]`.replace(" []", "");
}

function playRow(p) {
  const status = playStatus(p);
  const diff = [p.play.keys ? `${Math.round(p.play.keys)}K` : null, p.play.version].filter(Boolean).join(" ");
  const acc = typeof p.play.accuracy === "number" && p.play.accuracy > 0 ? `${p.play.accuracy.toFixed(2)}%` : null;
  const mods = modList(p.play.mods);
  const holdable = ["waiting_replay", "send"].includes(p.phase);
  const removable = holdable || ["held", "failed", "unbound"].includes(p.phase);
  const play = (what) => () => action(`plays/${encodeURIComponent(p.id)}/${what}`);

  const name = `${p.play.artist ?? ""} - ${chartName(p.play)}`;
  return el("li", { class: `play ${status.kind}` },
    cover(p),
    el("div", { class: "text" },
      p.url
        ? el("a", { class: "title", href: p.url, target: "_blank", rel: "noreferrer", title: `${name}\nOpen on Mania Tracker` }, p.play.title ?? "Unknown")
        : el("span", { class: "title", title: name }, p.play.title ?? "Unknown"),
      p.play.artist && el("span", { class: "artist" }, p.play.artist),
      el("div", { class: "facts" },
        diff && el("span", {}, diff),
        p.play.client === "lazer" && el("span", {}, "lazer"),
        mods.length > 0 && el("span", { class: "mods" }, mods.map(modBadge)),
        acc && el("span", {}, acc)),
      (status.detail || removable) && el("span", { class: "detail" }, status.detail && el("span", {}, status.detail),
        holdable && el("button", { class: "link act", disabled: slow, onclick: play(p.onHold ? "send" : "hold") }, p.onHold ? "Send" : "Hold"),
        removable && el("button", { class: "link act", disabled: slow, onclick: play("remove") }, "Remove")),
      rating(p.rating),
    ),
    el("div", { class: "side" },
      stamp(status, p.id),
      el("span", { class: "when" }, time(p.capturedAt)),
    ),
  );
}

// the site's dan labels are bare: "8+" is the 8+ dan, "gamma-" stays as it is
function danName(d) {
  const name = /^\d/.test(d.label) ? `${d.label} dan` : d.label;
  return d.ln ? `LN ${name}` : name;
}

function rating(r) {
  if (!r) return null;
  return el("div", { class: "rating" },
    typeof r.msd === "number" && el("span", { class: "msd" }, el("b", {}, r.msd.toFixed(2)), " MSD"),
    r.dan && el("span", {
      class: `dan${r.dan.eligible ? "" : " off"}`,
      title: r.dan.eligible ? null : "This play doesn't count toward your dan",
    }, danName(r.dan)));
}

const SHOWN = 50;
function playList(plays) {
  const shown = plays.filter((p, i) => i < SHOWN || ["spin", "wait"].includes(playStatus(p).kind));
  const rest = plays.length - shown.length;
  const ids = new Set(shown.map((p) => p.id));
  for (const cache of [covers, stamped, stampAt]) {
    for (const id of cache.keys()) if (!ids.has(id)) cache.delete(id);
  }
  return el("ul", { class: "plays" }, shown.map(playRow),
    rest > 0 && el("li", { class: "earlier" }, `and ${rest} earlier ${rest === 1 ? "play" : "plays"}`));
}

const PRESENCE_NOTES = {
  off: "Shown on your profile while osu! is open.",
  unsupported: "Mania Tracker doesn't support this yet.",
  needs_reconnect: "Mania Tracker needs your OK first.",
  sharing: "Shown on your profile while osu! is open.",
  paused: "Shown on your profile once osu! is open.",
};

function tosuAutostartNote(on, launch) {
  if (launch === "with-osu") return "tosu opens with osu! here.";
  if (snapshot.getTosu && !snapshot.getTosu.step) {
    return ["Couldn't find tosu on this computer. ", el("button", { class: "link", disabled: slow, onclick: () => action("get-tosu") }, "Get tosu")];
  }
  if (!on) return null;
  if (launch === "missing") return "Couldn't find tosu on this computer.";
  if (launch === "failed") return "tosu didn't start.";
  return null;
}

// cached so the page opens in the right color before the helper answers
const DEFAULT_THEME = { hue: 333, sat: 100 };
const THEME_KEY = "mania-bridge-theme";
const PRESET_HUES = [
  [333, "Pink"], [355, "Rose"], [15, "Red"], [30, "Orange"], [50, "Amber"], [95, "Lime"],
  [140, "Green"], [170, "Teal"], [200, "Cyan"], [225, "Blue"], [260, "Indigo"], [290, "Purple"],
];
const RAINBOW = "linear-gradient(to right, hsl(0,95%,55%), hsl(30,95%,55%), hsl(60,95%,50%), hsl(120,70%,45%), hsl(170,75%,45%), hsl(200,85%,50%), hsl(240,90%,60%), hsl(280,80%,55%), hsl(320,90%,55%), hsl(360,95%,55%))";

// don't re-render under a held slider, snapshots wait for the release
let themeDraft = null;
let renderWaiting = false;

function applyTheme({ hue, sat }) {
  document.documentElement.style.setProperty("--hue", hue);
  document.documentElement.style.setProperty("--sat", sat / 100);
  try { localStorage.setItem(THEME_KEY, JSON.stringify({ hue, sat })); } catch {}
}

function isDefaultTheme({ hue, sat }) {
  return hue === DEFAULT_THEME.hue && sat === DEFAULT_THEME.sat;
}

function pickTheme(theme) {
  if (busy) return;
  // so the next redraw doesn't flash the old value
  snapshot.theme = theme;
  applyTheme(theme);
  action(`theme/${theme.hue}/${theme.sat}`);
}

function themeStrip(label, key, max, gradient) {
  const theme = snapshot.theme;
  const send = (e) => {
    themeDraft = null;
    renderWaiting = false;
    pickTheme({ ...theme, [key]: Number(e.target.value) });
  };
  // no change event if it didn't move. setTimeout so a change event goes first
  const release = () => setTimeout(() => {
    if (!themeDraft) return;
    themeDraft = null;
    if (renderWaiting) {
      renderWaiting = false;
      render();
    }
  });
  return el("input", {
    type: "range", class: "strip", min: 0, max, value: theme[key], "aria-label": label, disabled: slow,
    style: `--track: ${gradient}`,
    onpointerdown: () => (themeDraft = { ...theme }),
    oninput: (e) => {
      themeDraft = { ...(themeDraft ?? theme), [key]: Number(e.target.value) };
      applyTheme(themeDraft);
    },
    onchange: send,
    onpointerup: release,
    onpointercancel: release,
  });
}

function themeSetting() {
  const theme = snapshot.theme ?? DEFAULT_THEME;
  return el("div", { class: "setting theme" },
    el("div", { class: "text" },
      el("span", { class: "title" }, "Theme color"),
      el("div", { class: "swatches" }, PRESET_HUES.map(([hue, name]) => el("button", {
        class: `swatch${hue === theme.hue ? " on" : ""}`, title: name, "aria-label": name,
        "aria-pressed": String(hue === theme.hue), disabled: slow,
        style: `background: hsl(${hue}, ${theme.sat}%, 65%)`,
        onclick: () => pickTheme({ ...theme, hue }),
      }))),
      themeStrip("Theme hue", "hue", 359, RAINBOW),
      themeStrip("Theme saturation", "sat", 100,
        `linear-gradient(to right, hsl(${theme.hue},0%,2%), hsl(${theme.hue},3%,8%) 22%, hsl(${theme.hue},18%,18%) 48%, hsl(${theme.hue},55%,56%) 76%, hsl(${theme.hue},100%,70%))`)),
    el("button", { class: "link reset", disabled: slow || isDefaultTheme(theme), onclick: () => pickTheme(DEFAULT_THEME) }, "Reset"));
}

function getUpdate() {
  action(snapshot.update.install ? "update/install" : "update/download");
}

function update() {
  const u = snapshot.update;
  if (!u || (u.dismissed && !u.step)) return null;
  const version = el("b", {}, u.version);
  if (u.step === "downloading" || u.step === "installing") {
    return el("div", { class: "pinned" },
      el("p", {}, u.step === "installing"
        ? ["Installing ", version, ", Mania Bridge opens again in a moment."]
        : ["Downloading ", version, u.percent != null ? ` · ${u.percent}%` : "", "…"]),
      el("i", { class: "spinner", style: `animation-delay: -${Date.now() % SPIN_MS}ms` }));
  }
  if (u.step === "failed") {
    return el("div", { class: "pinned failed" },
      el("p", {}, "Couldn't update to ", version, ". ", el("span", { class: "why" }, u.error)),
      el("button", { class: "key go", disabled: slow, onclick: () => action("update/install") }, "Try again"),
      el("button", { class: "link", disabled: slow, onclick: () => action("update/download") }, "Download"));
  }
  return el("div", { class: "pinned" },
    el("p", {}, "Mania Bridge ", version, " is out."),
    el("button", { class: "key go", disabled: slow, onclick: getUpdate }, u.install ? "Update" : "Download"),
    el("button", { class: "link", disabled: slow, onclick: () => action("update/later") }, "Later"));
}

let settingsOpen = false;

function toggle(title, on, path, note, cls = "") {
  return el("div", { class: `setting${cls}` },
    el("div", { class: "text" },
      el("span", { class: "title" }, title),
      note && el("span", { class: "line" }, note)),
    el("button", {
      class: `switch${on ? " on" : ""}`,
      role: "switch",
      "aria-checked": String(on),
      "aria-label": title,
      disabled: slow,
      onclick: () => action(`${path}/${on ? "off" : "on"}`),
    }, el("i")));
}

// plays, live status or both: never neither, then the app would do nothing
let lockedHint = null;

function sharing() {
  const p = snapshot.presence;
  const plays = snapshot.sendPlays;
  const live = p.enabled;
  const liveAvailable = p.status !== "unsupported";
  const liveNote = p.status === "error" ? p.message : PRESENCE_NOTES[p.status];
  const hint = "Keep one of them on, or nothing is sent.";
  const row = (key, title, on, locked, can, next, line, cls) => el("div", { class: `setting${lockedHint === key ? " is-error" : cls}` },
    el("div", { class: "text" },
      el("span", { class: "title" }, title),
      lockedHint === key ? el("span", { class: "line" }, hint) : line && el("span", { class: "line" }, line)),
    el("button", {
      class: `switch${on ? " on" : ""}${locked ? " locked" : ""}`,
      role: "switch",
      "aria-checked": String(on),
      "aria-label": title,
      disabled: slow || (!on && !can),
      onclick: () => {
        if (locked) {
          lockedHint = key;
          return render();
        }
        action(`sharing/${next}`);
      },
    }, el("i")));
  return [
    row("plays", "Send my plays", plays, plays && !(live && liveAvailable), true, plays ? "live" : live ? "both" : "plays",
      !plays && "Off. The maps you finish aren't sent.", ""),
    row("live", "Share my live status", live, live && !plays, liveAvailable, live ? "plays" : plays ? "both" : "live",
      live ? liveNote : p.status === "unsupported" && liveNote, live ? ` is-${p.status}` : ""),
  ];
}

function settings(c, host, reconnect) {
  const t = snapshot.tosuAutostart;
  const { launch } = snapshot.tosu;
  const tosuNote = tosuAutostartNote(t, launch);
  const computer = [
    t != null && toggle("Open tosu with Mania Bridge", t, "tosu-autostart", tosuNote,
      t && ["missing", "failed"].includes(launch) ? " is-error" : ""),
    snapshot.autostart != null && toggle("Open Mania Bridge on startup", snapshot.autostart, "autostart"),
    snapshot.startInTray != null && toggle("Start in the tray", snapshot.startInTray, "start-in-tray"),
  ].filter(Boolean);
  return el("div", { class: "settings" },
    el("div", { class: "head" },
      el("p", { class: "say" }, "Settings"),
      el("button", { class: "link", onclick: toggleSettings }, "Back to plays")),
    unsaved && el("p", { class: "status bad" }, unsaved),
    el("p", { class: "group" }, "Mania Tracker"),
    sharing(),
    el("div", { class: "setting" },
      el("div", { class: "text" },
        el("span", { class: "title" }, "Connected as ", c.username),
        host && el("span", { class: "line" }, host)),
      reconnect
        ? el("button", { class: "key go", disabled: slow, onclick: () => action("connect") }, "Connect again")
        : el("button", { class: "key danger", disabled: slow, onclick: () => action("disconnect") }, "Disconnect")),
    computer.length > 0 && [el("p", { class: "group" }, "This computer"), ...computer],
    snapshot.theme && [el("p", { class: "group" }, "Look"), themeSetting()],
    el("p", { class: "about" }, `Mania Bridge ${snapshot.version}`,
      snapshot.update && !snapshot.update.step
        && [" · ", el("button", { class: "link", disabled: slow, onclick: getUpdate }, `${snapshot.update.install ? "Update to" : "Get"} ${snapshot.update.version}`)]));
}

function toggleSettings() {
  settingsOpen = !settingsOpen;
  lockedHint = null;
  render();
}

function statusLine(c) {
  switch (c.status) {
    case "offline": return el("p", { class: "status warn" }, c.message ?? "Mania Tracker is not reachable.");
    case "not_in_beta": return el("p", { class: "status warn" }, c.message ?? "This account is not in the beta.");
    case "revoked":
    case "error": return el("p", { class: "status bad" }, c.message ?? "This connection no longer works.");
    case "disabled": return el("p", { class: "status warn" }, c.message);
    default: return null;
  }
}

function now() {
  const { tosu } = snapshot;
  const line = (state, name, ...rest) => el("div", { class: `now ${state}` }, el("i"), el("b", {}, name), rest);
  if (!tosu.connected) return line(tosu.launch === "with-osu" ? "off" : "missing", "tosu", tosuDetail(tosu));
  if (tosu.blocked) return line("missing", "osu!", "tosu can't read it");
  if (!tosu.osu) return line("off", "osu!", "Not open");
  return line("on", gameName(tosu), osuLabel(tosu), el("span", { class: "as" }, "as ", el("b", {}, tosu.player || "Guest")));
}

function waiting() {
  const node = el("div", { class: "blank" });
  node.innerHTML = `<svg width="100%" height="52" aria-hidden="true"><g class="still">
    <rect class="dashed" x="1" y="1" width="72" height="50" rx="8"/>
    <path class="thick" d="M88 12h96"/>
    <path d="M88 26h58M88 41h26M124 41h18M152 41h30"/>
    <svg x="100%" overflow="visible">
      <rect class="dashed" x="-84" y="13" width="78" height="24" rx="6" transform="rotate(-5 -45 25)"/>
    </svg>
  </g></svg>`;
  return node;
}

// reuse the same <img>, a new one is blank until it decodes
let avatar = null;

function face(c) {
  if (!c.avatarUrl) return c.username[0].toUpperCase();
  if (avatar?.getAttribute("src") !== c.avatarUrl) avatar = el("img", { src: c.avatarUrl, alt: "" });
  return avatar;
}

function home(c, host) {
  const reconnect = c.status === "revoked" || c.status === "error";
  return el("div", { class: "home" },
    el("div", { class: "me" },
      el("span", { class: "face" }, face(c)),
      el("div", { class: "who" },
        el("div", { class: "name" },
          el("a", {
            class: "profile",
            href: `${c.origin}/player/${encodeURIComponent(c.username)}`,
            target: "_blank",
            rel: "noreferrer",
            title: "Open your profile on Mania Tracker",
            style: c.accent && `color: ${c.accent}`,
          }, c.username),
          host && el("span", {}, host)),
        now())),
    update(),
    tosuHint(),
    statusLine(c),
    reconnect && !settingsOpen
      && el("button", { class: "key go", disabled: slow, onclick: () => action("connect") }, "Connect again"),
    helperDown && el("p", { class: "status warn" }, "Lost the bridge helper. Reconnecting."),
    settingsOpen
      ? settings(c, host, reconnect)
      : snapshot.plays.length
        ? playList(snapshot.plays)
        : el("div", { class: "nothing" },
            el("p", { class: "say" }, snapshot.sendPlays ? "No plays yet." : "Sending plays is off."),
            el("p", { class: "lead" }, snapshot.sendPlays
              ? "Finish a mania map and it shows up here."
              : "The maps you finish stay off your profile. Turn it back on in the settings."),
            waiting(), el("i"), el("i"), el("i")));
}

const GEAR = `<path d="M4.10 0.39L5.90 0.39L6.07 1.88L7.16 2.51L8.55 1.92L9.44 3.47L8.24 4.37L8.24 5.63L9.44 6.53L8.55 8.08L7.16 7.49L6.07 8.12L5.90 9.61L4.10 9.61L3.93 8.12L2.84 7.49L1.45 8.08L0.56 6.53L1.76 5.63L1.76 4.37L0.56 3.47L1.45 1.92L2.84 2.51L3.93 1.88z" stroke-linejoin="round"/><circle cx="5" cy="5" r="1.4"/>`;

function render() {
  if (themeDraft) {
    renderWaiting = true;
    return;
  }
  app.replaceChildren();
  if (helperDown && !snapshot) {
    app.append(el("div", { class: "connect full" }, el("p", { class: "note" }, "The bridge helper is not running.")));
    return;
  }
  if (!snapshot) return;

  const c = snapshot.connection;
  const host = c.origin === OFFICIAL_ORIGIN ? null : c.origin.replace(/^https?:\/\//, "");
  if (snapshot.theme) applyTheme(snapshot.theme);
  const ownTheme = snapshot.theme && !isDefaultTheme(snapshot.theme);
  document.documentElement.style.setProperty("--accent", (!ownTheme && c.accent) || "var(--osu-pink)");
  const hasAccount = c.username && !["disconnected", "connecting"].includes(c.status);
  if (c.status === "connected" && hasAccount && wasConnecting && !STILL.matches) {
    crossAt = Date.now();
    if (document.hidden) document.addEventListener("visibilitychange", () => crossAt && (cross(), render()), { once: true });
    else cross();
  }
  if (!hasAccount) crossAt = 0;
  wasConnecting = c.status === "connecting";
  const atHome = hasAccount && !crossAt;
  if (!atHome) settingsOpen = false;

  const own = !snapshot.titlebar && location.origin === BRIDGE;
  if (own || atHome) {
    app.append(el("div", { class: "corner" },
      atHome && el("button", { class: `gear${settingsOpen ? " on" : ""}`, title: "Settings", "aria-pressed": String(settingsOpen), onclick: toggleSettings }, icon(GEAR)),
      own && snapshot.tray && el("button", { title: "Hide to tray", onclick: () => windowAction("hide") }, icon(`<path d="M0 5.5h10"/>`)),
      own && el("button", { class: "close", title: "Quit", onclick: () => windowAction("close") }, icon(`<path d="m.5.5 9 9m0-9-9 9"/>`))));
  }

  if (atHome) {
    app.append(el("div", { class: "ground" }, home(c, host), shore()));
    return;
  }

  const connecting = c.status === "connecting";
  const full = !snapshot.plays.length;
  const river = crossAt ? [
    el("p", { class: "say" }, "Connected as ", c.username, "."),
    el("p", { class: "lead" }, "Every mania play you finish goes to your profile from here on."),
  ] : [
    el("p", { class: "say" }, connecting ? "Halfway there." : "Not connected yet."),
    el("p", { class: "lead" }, connecting
      ? "Finish connecting on the page that opened in your browser."
      : "Nothing you play gets across. Connect and every mania play you finish goes to your profile on its own."),
    !snapshot.tosu.connected && !["starting", "with-osu"].includes(snapshot.tosu.launch)
      && el("p", { class: "field-hint" }, "Plays are only sent while tosu is open."),
    blockedHint(),
    el("div", { class: "cross" },
      connecting
        ? el("button", { class: "primary waiting", disabled: true }, "Waiting for the browser")
        : el("button", { class: "primary", disabled: slow, onclick: () => action("connect") }, "Connect to Mania Tracker"),
      host && el("span", { class: "host" }, host)),
    connecting
      ? el("p", { class: "note" },
          c.authorizeUrl && el("a", { href: c.authorizeUrl, target: "_blank", rel: "noreferrer" }, "Open the page again"),
          c.authorizeUrl && " · ",
          el("button", { class: "link", onclick: () => action("cancel") }, "Cancel"))
      : c.message && el("p", { class: "note" }, c.message),
    update(),
    getTosu(),
  ];
  app.append(el("div", { class: `connect${full ? " full" : ""}` },
    intro(crossAt ? "flat" : connecting ? "down" : "up"),
    el("div", { class: "river" }, river, full && riverbed())));

  if (helperDown) app.append(el("p", { class: "status warn" }, "Lost the bridge helper. Reconnecting."));
  if (snapshot.plays.length) app.append(playList(snapshot.plays));
}

function windowAction(name) {
  fetch(`${BRIDGE}/window/${name}`, { method: "POST" }).catch(() => {});
}

function icon(paths) {
  const node = el("span", { class: "icon" });
  node.innerHTML = `<svg viewBox="0 0 10 10" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1">${paths}</svg>`;
  return node;
}

// windows: no system frame, so draw a title bar
let titlebar = null;
function showTitlebar() {
  if (titlebar || !snapshot?.titlebar || location.origin !== BRIDGE) return;
  titlebar = el("header", {
    class: "titlebar",
    onmousedown: (e) => e.button === 0 && !e.target.closest("button") && windowAction("drag"),
  },
    el("img", { src: "images/mania-tracker.png", alt: "", width: 16, height: 16 }),
    el("span", { class: "name" }, "Mania Bridge"),
    el("button", { class: "tray", title: "Hide to tray", onclick: () => windowAction("hide") },
      icon(`<path d="M5 .5v6M2.5 4 5 6.5 7.5 4M.5 9.5h9"/>`)),
    el("button", { title: "Minimize", onclick: () => windowAction("minimize") }, icon(`<path d="M0 5.5h10"/>`)),
    el("button", { class: "close", title: "Close", onclick: () => windowAction("close") }, icon(`<path d="m.5.5 9 9m0-9-9 9"/>`)),
  );
  document.body.prepend(titlebar);
  document.documentElement.classList.add("framed");
  addEventListener("blur", () => document.documentElement.classList.add("inactive"));
  addEventListener("focus", () => document.documentElement.classList.remove("inactive"));
}

function listen() {
  const events = new EventSource(`${BRIDGE}/events`);
  events.onmessage = (ev) => {
    snapshot = JSON.parse(ev.data);
    helperDown = false;
    showTitlebar();
    render();
  };
  events.addEventListener("reload", () => location.reload());
  events.onerror = () => {
    helperDown = true;
    render();
  };
}

// animated ink wobble. ink1-3 are static variants so neighbouring rows differ
const INK = 2.6;
const INK_MS = 700;

function inkFilter() {
  document.body.insertAdjacentHTML("beforeend", `<svg class="defs" width="0" height="0" aria-hidden="true">
    <filter id="ink" x="-5%" y="-5%" width="110%" height="110%" color-interpolation-filters="sRGB">
      <feTurbulence type="fractalNoise" baseFrequency="0.04" numOctaves="2" seed="1" result="from"/>
      <feTurbulence type="fractalNoise" baseFrequency="0.04" numOctaves="2" seed="2" result="to"/>
      <feComposite in="from" in2="to" operator="arithmetic" k1="0" k2="1" k3="0" k4="0" result="noise"/>
      <feDisplacementMap in="SourceGraphic" in2="noise" scale="${INK}" xChannelSelector="R" yChannelSelector="G"/>
    </filter>${[1, 2, 3].map((n) => `
    <filter id="ink${n}" x="-5%" y="-5%" width="110%" height="110%" color-interpolation-filters="sRGB">
      <feTurbulence type="fractalNoise" baseFrequency="0.04" numOctaves="2" seed="${n * 13}" result="noise"/>
      <feDisplacementMap in="SourceGraphic" in2="noise" scale="${INK}" xChannelSelector="R" yChannelSelector="G"/>
    </filter>`).join("")}</svg>`);
  const [from, to] = document.querySelectorAll("#ink feTurbulence");
  const mix = document.querySelector("#ink feComposite");
  const push = document.querySelector("#ink feDisplacementMap");
  let seed = 2;
  let start = 0;
  let last = 0;
  const frame = (now) => {
    requestAnimationFrame(frame);
    // 30fps
    if (now - last < 30 || STILL.matches || !document.querySelector(".scene, .opening")) return;
    last = now;
    let t = (now - start) / INK_MS;
    if (t >= 1) {
      from.setAttribute("seed", seed);
      to.setAttribute("seed", (seed = seed % 999 + 1));
      start = now;
      t = 0;
    }
    mix.setAttribute("k2", 1 - t);
    mix.setAttribute("k3", t);
    // averaging two patterns flattens it, compensate
    push.setAttribute("scale", INK / Math.hypot(1 - t, t));
  };
  requestAnimationFrame(frame);
}

// ?done: where the browser lands after the consent page
function donePage(ok) {
  document.documentElement.classList.add("done");
  crossAt = Date.now();
  crossNoteMs = 4200;
  reach = Math.ceil(Math.max(screen.width, innerWidth) / 200) * 100;
  app.append(el("div", { class: "connect full" },
    bridge({
      osu: { state: "ok", name: "osu!" },
      tosu: { state: "ok", name: "tosu" },
      site: ok
        ? { state: "ok", name: "Mania Tracker", detail: "Connected" }
        : { state: "idle", name: "Mania Tracker", detail: "Not connected" },
    }, ok ? "flat" : "up"),
    el("div", { class: "river" },
      el("p", { class: "say" }, ok ? "Connected." : "Nothing was connected."),
      el("p", { class: "lead" }, "You can close this tab."),
      riverbed())));
}

let resized = 0;
addEventListener("resize", () => {
  clearTimeout(resized);
  resized = setTimeout(() => document.querySelector(".shore") && render(), 200);
});

// overlay scrollbar, the page still scrolls natively
function scrollbar() {
  const bar = el("div", { class: "scrollbar", hidden: "" }, el("i"));
  const thumb = bar.firstChild;
  document.body.append(bar);
  const page = () => document.documentElement.classList.contains("framed") ? app : document.scrollingElement;
  const MIN = 40;
  const update = () => {
    const p = page();
    const over = p.scrollHeight - p.clientHeight;
    bar.hidden = over <= 1;
    if (bar.hidden) return;
    const track = bar.clientHeight;
    const size = Math.max(MIN, track * p.clientHeight / p.scrollHeight);
    thumb.style.height = `${size}px`;
    thumb.style.top = `${(track - size) * p.scrollTop / over}px`;
  };
  bar.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    const p = page();
    const track = bar.clientHeight;
    const size = thumb.offsetHeight;
    const per = (p.scrollHeight - p.clientHeight) / Math.max(1, track - size);
    if (e.target !== thumb) p.scrollTop = (e.clientY - bar.getBoundingClientRect().top - size / 2) * per;
    const from = e.clientY;
    const start = p.scrollTop;
    bar.setPointerCapture(e.pointerId);
    bar.classList.add("dragging");
    const move = (m) => { p.scrollTop = start + (m.clientY - from) * per; };
    const up = () => {
      bar.classList.remove("dragging");
      bar.removeEventListener("pointermove", move);
    };
    bar.addEventListener("pointermove", move);
    bar.addEventListener("pointerup", up, { once: true });
    bar.addEventListener("pointercancel", up, { once: true });
  });
  let queued = false;
  const soon = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => { queued = false; update(); });
  };
  document.addEventListener("scroll", soon, true);
  addEventListener("resize", soon);
  new MutationObserver(soon).observe(app, { childList: true, subtree: true });
  new ResizeObserver(soon).observe(app);
}

const OPENING_MS = 1300;

async function openingScreen() {
  document.documentElement.classList.add("alone");
  if (!STILL.matches) {
    const node = el("div", { class: "opening" });
    node.innerHTML = `<svg viewBox="0 0 160 160" width="160" height="160" aria-hidden="true"><g class="ink">
      <clipPath id="opening-disc"><circle cx="80" cy="80" r="70"/></clipPath>
      <circle class="disc" cx="80" cy="80" r="70"/>
      <g clip-path="url(#opening-disc)">
        <path class="arrow" d="M50 39h29V23l32 31-32 31V69H50z"/>
        <path class="line spark" d="M-14-4l-7-8M0-9v-11M14-4l7-8"/>
        <path class="deck" d="M-10 103H19Q80 74 141 103H170V147H111A31 33 0 0 0 49 147H-10z"/>
        <path class="water" d="M-60 131q10-3 20 0${"t20 0".repeat(13)}V170H-60z" style="${loop(4000)}"/>
      </g>
      <circle class="rim" cx="80" cy="80" r="70"/>
    </g></svg>`;
    document.body.append(node);
    await new Promise((resolve) => setTimeout(resolve, OPENING_MS));
    await node.animate({ opacity: [1, 0] }, { duration: 240, easing: "steps(4)", fill: "forwards" }).finished;
  }
  windowAction("opened");
}

const done = new URLSearchParams(location.search).get("done");
try { applyTheme(JSON.parse(localStorage.getItem(THEME_KEY)) ?? DEFAULT_THEME); } catch {}
inkFilter();
scrollbar();
if (done) donePage(done === "yes");
else if (new URLSearchParams(location.search).has("opening")) openingScreen();
else {
  listen();
  render();
}
