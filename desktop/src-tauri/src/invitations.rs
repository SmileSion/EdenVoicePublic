use std::sync::Mutex;
use tauri::{Manager, AppHandle};
use tauri_plugin_deep_link::DeepLinkExt;

#[derive(Default)]
pub struct Invitations(Mutex<Option<String>>);

pub fn parse_invitation(raw:&str)->Option<String> {
 if raw.len()>1024 || raw.bytes().any(|b| b<=32||b==127) {return None}
 let u=url::Url::parse(raw).ok()?;
 if u.scheme()!="edenvoice"||u.host_str()!=Some("join")||!u.username().is_empty()||u.password().is_some()||u.port().is_some()||u.fragment().is_some()||!matches!(u.path(),""|"/"){return None}
 let pairs:Vec<_>=u.query_pairs().collect();
 if pairs.len()!=1||pairs[0].0!="address" {return None}
 let address=pairs[0].1.as_ref();
 if address.len()>256||address.bytes().any(|b|b<=32||b==127||b"@/?#%\\".contains(&b)){return None}
 let (host,port)=address.rsplit_once(':')?;
 if port.is_empty()||port.len()>5||!port.bytes().all(|b|b.is_ascii_digit())||port.parse::<u16>().ok()?==0{return None}
 if host.starts_with('[') {host.strip_prefix('[')?.strip_suffix(']')?.parse::<std::net::Ipv6Addr>().ok()?;} else if host.split('.').any(|p|p.is_empty()||p.len()>63||p.starts_with('-')||p.ends_with('-')||!p.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-')){return None}
 Some(format!("{}:{}",host.to_ascii_lowercase(),port.parse::<u16>().ok()?))
}
fn receive(app:&AppHandle, raw:&str){
 if let Some(address)=parse_invitation(raw){
  *app.state::<Invitations>().0.lock().unwrap()=Some(address);
  if let Some(w)=app.get_webview_window("main"){let _=w.show();let _=w.unminimize();let _=w.set_focus();}
 }
}
pub fn setup(app:&AppHandle)->tauri::Result<()> {
 app.manage(Invitations::default());
 #[cfg(any(target_os="linux",target_os="windows"))]
 if let Err(error)=app.deep_link().register_all(){eprintln!("invitation URI registration unavailable: {error}");}
 if let Ok(Some(urls))=app.deep_link().get_current(){for u in urls{receive(app,u.as_str());}}
 let handle=app.clone();app.deep_link().on_open_url(move |event|{for u in event.urls(){receive(&handle,u.as_str());}});
 Ok(())
}
pub fn has_pending(app:&AppHandle)->bool {app.state::<Invitations>().0.lock().unwrap().is_some()}
#[tauri::command]
pub fn take_server_invitation(window:tauri::WebviewWindow,app:AppHandle)->Option<String>{
 if window.label()!="main" {return None}
 app.state::<Invitations>().0.lock().unwrap().take()
}
#[cfg(test)]mod tests{use super::*;
 #[test]fn accepts_addresses(){assert_eq!(parse_invitation("edenvoice://join?address=voice.example.com%3A7000"),Some("voice.example.com:7000".into()));assert_eq!(parse_invitation("edenvoice://join?address=%5B%3A%3A1%5D%3A7000"),Some("[::1]:7000".into()));}
 #[test]fn rejects_unsafe_links(){for s in ["https://join?address=x:7000","edenvoice://join?address=x:0","edenvoice://join?address=x:7000&password=secret","edenvoice://join?address=x:7000&address=y:7000","edenvoice://join?address=x:7000#extra","edenvoice://evil@join?address=x:7000","edenvoice://join?address=x%0a:7000","edenvoice://join?address=bad/path:7000"]{assert!(parse_invitation(s).is_none(),"{s}");}}
}
