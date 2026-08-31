/* The remote-mode shim (ADR-0021, spec docs/SPECS/active/2026-08-31-remote-control.md).
 *
 * Served by the bridge ahead of app.js. When the frontend is running in a phone's
 * browser there is no Tauri, so this defines window.__TAURI__ with the exact three
 * surfaces app.js touches — core.invoke, event.listen, dialog — and redirects every
 * one of them over a WebSocket to the desktop app. app.js is unchanged apart from
 * the few places that must know native services are absent (window.__BLINKVIEW_REMOTE__).
 *
 * Frames (see remote.rs): requests are {"id", "cmd", "args"}; replies {"id","ok",
 * "result"|"err"}; pushed events {"ev","payload"}. Requests are queued until the
 * socket opens and replayed after a reconnect, so a Wi-Fi blip fails in flight but
 * never loses a click made while offline. */
(() => {
  if (window.__TAURI__ || window.__BLINKVIEW_REMOTE__) return; // native window, or already shimmed
  window.__BLINKVIEW_REMOTE__ = true;

  const pending = new Map();     // id -> {res, rej}
  const subs = new Map();        // event name -> Set<handler>
  let ws = null;
  let nextId = 0;
  const backlog = [];

  const rawSend = (obj) => { try { ws.send(JSON.stringify(obj)); return true; } catch { return false; } };
  const send = (obj) => { if (!ws || ws.readyState !== 1) { backlog.push(obj); return; } rawSend(obj); };

  const notify = (name, msg) => {
    const set = subs.get(name);
    if (set) for (const h of [...set]) { try { h(msg); } catch (e) { console.error(e); } }
  };

  const connect = () => {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    let sock;
    try { sock = new WebSocket(proto + "//" + location.host + "/ws"); }
    catch { setTimeout(connect, 2000); return; }
    sock.onopen = () => {
      ws = sock;
      backlog.splice(0).forEach(rawSend);
      notify("blinkview:remote-connected", {});
    };
    sock.onmessage = (e) => {
      let m; try { m = JSON.parse(e.data); } catch { return; }
      if (m.id != null) {
        const p = pending.get(m.id);
        if (!p) return;
        pending.delete(m.id);
        if (m.ok) p.res(m.result);
        else p.rej(new Error(m.err || "remote command failed"));
      } else if (m.ev) {
        notify(m.ev, { event: m.ev, payload: m.payload });
      }
    };
    sock.onclose = () => {
      if (ws === sock) ws = null;
      for (const p of pending.values()) p.rej(new Error("remote connection lost"));
      pending.clear();
      notify("blinkview:remote-lost", {});
      setTimeout(connect, 1500); // reconnect forever; the token cookie still gates us
    };
    sock.onerror = () => { try { sock.close(); } catch { /* closing twice is fine */ } };
  };
  connect();

  const unlisten = (name, h) => () => { const set = subs.get(name); if (set) set.delete(h); };
  window.__TAURI__ = {
    core: {
      invoke(cmd, args) {
        return new Promise((res, rej) => {
          const id = ++nextId;
          pending.set(id, { res, rej });
          send({ id, cmd, args: args ?? null });
        });
      },
      transformCallback: (cb) => cb,
    },
    event: {
      listen(name, h) { let set = subs.get(name); if (!set) subs.set(name, set = new Set()); set.add(h); return Promise.resolve(unlisten(name, h)); },
      once(name, h) { const g = (m) => { unlisten(name, g)(); h(m); }; let set = subs.get(name); if (!set) subs.set(name, set = new Set()); set.add(g); return Promise.resolve(() => set.delete(g)); },
    },
    dialog: {
      open: () => Promise.reject(new Error("Folder picking needs the desktop app")),
      save: () => Promise.reject(new Error("Saving needs the desktop app")),
      message: () => Promise.resolve(),
      ask: () => Promise.resolve(false),
      confirm: () => Promise.resolve(false),
    },
  };
})();
