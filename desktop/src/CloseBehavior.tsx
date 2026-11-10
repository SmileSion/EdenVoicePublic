import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Minimize2, Power, X } from "lucide-react";

type Behavior = "ask" | "tray" | "quit";
type Preferences = { behavior: Behavior; pending: boolean; trayAvailable: boolean };

export function CloseBehaviorSettings() {
  const [behavior, setBehavior] = useState<Behavior>("ask");
  const [busy, setBusy] = useState(true);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let received = false;
    void (async () => {
      unlisten = await listen<Behavior>("close-behavior-changed", event => {
        if (!disposed) { received = true; setBehavior(event.payload); }
      });
      if (disposed) { unlisten(); return; }
      const value = await invoke<Preferences>("get_close_preferences");
      if (!disposed && !received) setBehavior(value.behavior);
    })().catch(e => { if (!disposed) setError(String(e)); })
      .finally(() => { if (!disposed) setBusy(false); });
    return () => { disposed = true; unlisten?.(); };
  }, []);
  const save = async (value: Behavior) => {
    setBusy(true); setStatus(""); setError("");
    try {
      await invoke("set_close_behavior", { behavior: value });
      setBehavior(value); setStatus("已保存");
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <div className="close-behavior-settings">
    <h3>关闭主窗口</h3>
    <label htmlFor="close-behavior">点击关闭按钮时</label>
    <select id="close-behavior" value={behavior} disabled={busy} onChange={e => void save(e.target.value as Behavior)}>
      <option value="ask">每次询问</option>
      <option value="tray">最小化到托盘</option>
      <option value="quit">退出程序</option>
    </select>
    <p>修改后立即保存。收起到托盘会继续语音和观影；退出会结束连接。</p>
    {status && <span role="status">{status}</span>}
    {error && <p role="alert">{error}</p>}
  </div>;
}

export function CloseBehaviorDialog({ theme = "system" }: { theme?: "system" | "light" | "dark" }) {
  const [open, setOpen] = useState(false);
  const opened = useRef(false);
  const [trayAvailable, setTrayAvailable] = useState(true);
  const [choice, setChoice] = useState<"tray" | "quit">("tray");
  const [remember, setRemember] = useState(false);
  const [busy, setBusy] = useState(false);
  const working = useRef(false);
  const [error, setError] = useState("");
  const dialog = useRef<HTMLElement>(null);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let received = false;
    const show = (value: Preferences) => {
      if (disposed || !value.pending) return;
      setTrayAvailable(value.trayAvailable);
      if (!opened.current) {
        setChoice(value.trayAvailable ? "tray" : "quit");
        setRemember(false); setError("");
      }
      opened.current = true;
      setOpen(true);
    };
    void (async () => {
      unlisten = await listen<Preferences>("window-close-requested", event => {
        received = true; show(event.payload);
      });
      if (disposed) { unlisten(); return; }
      const value = await invoke<Preferences>("get_close_preferences");
      if (!received) show(value); // Also handles close requests before React started.
    })().catch(e => { if (!disposed) console.error("close behavior listener", e); });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  const resolve = async (action: "tray" | "quit" | "cancel") => {
    if (working.current) return;
    working.current = true; setBusy(true); setError("");
    try {
      await invoke("resolve_window_close", { action, remember: action !== "cancel" && remember });
      opened.current = false; setOpen(false);
    } catch (e) { setError(String(e)); }
    finally { working.current = false; setBusy(false); }
  };
  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement;
    dialog.current?.focus();
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault(); event.stopImmediatePropagation(); void resolve("cancel");
      }
      if (event.key === "Tab") {
        const elements = Array.from(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)') ?? []);
        const first = elements[0], last = elements[elements.length - 1];
        if (event.shiftKey && (document.activeElement === first || document.activeElement === dialog.current)) {
          event.preventDefault(); last?.focus();
        } else if (!event.shiftKey && (document.activeElement === last || document.activeElement === dialog.current)) {
          event.preventDefault(); first?.focus();
        }
      }
    };
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("keydown", key, true);
      if (previous instanceof HTMLElement) previous.focus();
    };
  }, [open]);

  if (!open) return null;
  return createPortal(<div className="close-choice-backdrop" data-theme={theme}>
    <section ref={dialog} tabIndex={-1} className="close-choice" role="dialog" aria-modal="true" aria-labelledby="close-choice-title" aria-describedby="close-choice-description">
      <header><h2 id="close-choice-title">关闭 EdenVoice</h2><button aria-label="取消关闭" disabled={busy} onClick={() => void resolve("cancel")}><X size={18}/></button></header>
      <p id="close-choice-description">你希望如何关闭窗口？</p>
      <label className={choice === "tray" ? "selected" : ""}>
        <input type="radio" name="close-choice" value="tray" checked={choice === "tray"} disabled={busy || !trayAvailable} onChange={() => setChoice("tray")}/>
        <Minimize2 size={20}/><span><strong>最小化到托盘</strong><small>保持语音连接和观影，点击托盘图标恢复</small></span>
      </label>
      <label className={choice === "quit" ? "selected" : ""}>
        <input type="radio" name="close-choice" value="quit" checked={choice === "quit"} disabled={busy} onChange={() => setChoice("quit")}/>
        <Power size={20}/><span><strong>退出程序</strong><small>断开语音连接并关闭本程序启动的播放器</small></span>
      </label>
      {!trayAvailable && <p className="close-choice-error">系统托盘暂不可用，可以取消关闭或退出程序。</p>}
      <label className="close-choice-remember"><input type="checkbox" checked={remember} disabled={busy} onChange={e => setRemember(e.target.checked)}/><span>不再提醒，将此选择设为默认行为</span></label>
      <p className="close-choice-hint">之后可在“设置 → 界面 → 关闭主窗口”修改。</p>
      {error && <p className="close-choice-error" role="alert">{error}</p>}
      <footer><button disabled={busy} onClick={() => void resolve("cancel")}>取消</button><button className="primary" disabled={busy || (choice === "tray" && !trayAvailable)} onClick={() => void resolve(choice)}>{busy ? "处理中…" : choice === "tray" ? "收起到托盘" : "退出程序"}</button></footer>
    </section>
  </div>, document.body);
}
