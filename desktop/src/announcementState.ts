export interface Announcement {revision:string;title:string;body:string;published_at:number}
export function validAnnouncement(value:unknown):value is Announcement {
 if(!value||typeof value!=='object')return false;
 const a=value as Announcement;
 return typeof a.revision==='string'&&/^[1-9]\d{0,19}$/.test(a.revision)&&typeof a.title==='string'&&a.title.length>0&&a.title.length<=160&&typeof a.body==='string'&&a.body.length>0&&a.body.length<=6000&&Number.isFinite(a.published_at);
}
export function readRevision(storage:Pick<Storage,'getItem'>,key:string):bigint{
 try{const v=storage.getItem(key);return v&&/^\d{1,20}$/.test(v)?BigInt(v):0n;}catch{return 0n;}
}
export function acknowledge(storage:Pick<Storage,'getItem'|'setItem'>,key:string,revision:string){
 const next=BigInt(revision),previous=readRevision(storage,key);
 storage.setItem(key,(next>previous?next:previous).toString());
}
export const announcementKey=(scope:string,client:string)=>`edenvoice.announcement.read:${encodeURIComponent(scope)}:${encodeURIComponent(client)}`;
