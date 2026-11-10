import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { getAvatarData } from "./bridge";

export const AvatarCacheContext = createContext(0);
const initials = (name: string) => Array.from(name.trim()).slice(0, 1).join("").toUpperCase() || "?";

export function Avatar({ avatar, name, size = 32, speaking = false, previewable = true, fallback, revision }: { avatar: string; name: string; size?: number; speaking?: boolean; previewable?: boolean; fallback?: ReactNode; revision?: number }) {
  const contextRevision = useContext(AvatarCacheContext);
  const cacheRevision = revision ?? contextRevision;
  const [source, setSource] = useState<string | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  useEffect(() => {
    let active = true;
    let retryTimer: number | undefined;
    setSource(null);
    setPreviewOpen(false);
    const load = async (attempt = 0) => {
      const data = await getAvatarData(avatar).catch(() => null);
      if (!active) return;
      if (data) {
        setSource(data);
      } else {
        const retryDelay = Math.min(4000, 100 * 2 ** attempt);
        retryTimer = window.setTimeout(() => void load(attempt + 1), retryDelay);
      }
    };
    if (avatar.startsWith("custom:")) void load();
    return () => { active = false; if (retryTimer !== undefined) window.clearTimeout(retryTimer); };
  }, [avatar, cacheRevision]);
  useEffect(() => {
    if (!previewOpen) return;
    const close = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape") setPreviewOpen(false); };
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [previewOpen]);
  const openPreview = (event: React.MouseEvent | React.KeyboardEvent) => {
    if (!previewable || !source) return;
    event.preventDefault();
    event.stopPropagation();
    setPreviewOpen(true);
  };
  return <>
    <span className={`avatar ${speaking ? "is-speaking" : ""} ${previewable && source ? "is-previewable" : ""}`} style={{ width: size, height: size }} role={previewable && source ? "button" : undefined} tabIndex={previewable && source ? 0 : undefined} aria-label={previewable && source ? `放大查看 ${name} 的头像` : undefined} onClick={openPreview} onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") openPreview(event); }}>
      {source ? <img draggable={false} onDragStart={event=>event.preventDefault()} src={source} alt="" /> : <span>{avatar && avatar.length <= 2 && !avatar.startsWith("custom:") ? avatar : fallback ?? initials(name)}</span>}
    </span>
    {previewOpen && source && createPortal(<div className="avatar-preview-backdrop" role="dialog" aria-modal="true" aria-label={`${name} 的头像`} onClick={(event) => { event.stopPropagation(); setPreviewOpen(false); }}><div className="avatar-preview-card" onClick={(event) => event.stopPropagation()}><img draggable={false} onDragStart={event=>event.preventDefault()} src={source} alt={`${name} 的头像`} /><strong>{name}</strong><span>点击空白处或按 Esc 关闭</span></div></div>, document.querySelector(".app-shell") ?? document.body)}
  </>;
}

