import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export function PlayerSettings() {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(true);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  useEffect(() => {
    let alive = true;
    invoke<{ mpvPath: string }>("get_watch_snapshot").then(view => { if (alive) setPath(view.mpvPath); })
      .catch(e => { if (alive) setError(String(e)); }).finally(() => { if (alive) setBusy(false); });
    return () => { alive = false; };
  }, []);
  const save = async (browse: boolean) => {
    setBusy(true); setError(""); setStatus("");
    try {
      const selected = browse ? await invoke<string | null>("select_mpv_path") : path.trim();
      if (!selected) return;
      const saved = await invoke<string>("save_mpv_path", { path: selected });
      setPath(saved); setStatus("已保存到本机，下次启动无需重新选择。正在播放的影片不受影响。");
    } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  return <div className="player-settings"><h3>观影播放器</h3>
    <p className="section-description">使用外部 mpv 播放本地影片，观影房间负责同步播放进度。</p>
    <label htmlFor="mpv-setting-path">mpv 可执行文件</label>
    <div className="watch-path"><input id="mpv-setting-path" value={path} disabled={busy} placeholder="Windows 选择 mpv.exe；macOS 可选择 mpv.app" onChange={e => { setPath(e.target.value); setStatus(""); }} /></div>
    <div className="watch-actions"><button disabled={busy} onClick={() => void save(true)}>选择播放器并保存</button><button disabled={busy || !path.trim()} onClick={() => void save(false)}>{busy ? "处理中…" : "保存播放器设置"}</button></div>
    <p className="section-description">此设置点击保存后立即生效。请选择已解压播放器目录中的 mpv.exe，不要选择安装程序、快捷方式或 mpv.com。请保留播放器目录内的依赖文件。</p>
    {status && <p role="status">{status}</p>}{error && <p role="alert" className="watch-error">{error}</p>}
  </div>;
}
