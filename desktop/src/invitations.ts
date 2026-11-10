export function normalizeServerAddress(raw:string):string {
 const value=raw.trim();
 if(!value||value.length>256||/[\s\x00-\x1f\x7f@/?#%\\]/.test(value))throw new Error('请输入有效的服务器地址和端口');
 const m=value.match(/^(\[[0-9a-fA-F:]+\]|[a-zA-Z0-9.-]+):(\d{1,5})$/);
 if(!m||Number(m[2])<1||Number(m[2])>65535)throw new Error('服务器地址需要有效端口，例如 voice.example.com:7000');
 if(!m[1].startsWith('[')&&m[1].split('.').some(p=>!p||p.length>63||p.startsWith('-')||p.endsWith('-')))throw new Error('服务器主机名无效');
 const parsed=new URL(`tcp://${value}`);return `${parsed.hostname.toLowerCase()}:${Number(m[2])}`;
}
export function parseInvitation(raw:string):string {
 if(raw.length>1024||/[\x00-\x20\x7f]/.test(raw))throw new Error('邀请链接无效');
 const u=new URL(raw);
 if(u.protocol!=='edenvoice:'||u.hostname!=='join'||u.username||u.password||u.port||u.hash||!['','/'].includes(u.pathname))throw new Error('仅支持 EdenVoice 服务器邀请链接');
 const pairs=[...u.searchParams.entries()];
 if(pairs.length!==1||pairs[0][0]!=='address')throw new Error('邀请链接只能包含服务器地址，不能包含密码');
 return normalizeServerAddress(pairs[0][1]);
}
export function createInvitation(address:string):string {return `edenvoice://join?address=${encodeURIComponent(normalizeServerAddress(address))}`;}
