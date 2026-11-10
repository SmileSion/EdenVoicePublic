// macOS-only probe. Default mode only reports API availability.
// --capture opens a local test window; capture requires clicking a button and
// explicitly choosing a source in the system picker. No microphone is captured.
// Run: swift -module-cache-path /tmp/eden-swift-module-cache scripts/screen-share-webkit-probe.swift
// Mirrors the current Wry media permission delegate and autoplay setting.
// API presence is not a substitute for permission/picker/capture verification.
import AppKit
import WebKit
import Network
final class Probe: NSObject, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandler {
 let capture = !CommandLine.arguments.contains("--check") && (CommandLine.arguments.contains("--capture") || (Bundle.main.bundleIdentifier?.hasPrefix("dev.edenvoice.capture-probe") ?? false))
 var window:NSWindow!
 var web:WKWebView!
 var listener:NWListener?
 func run(){
  let config=WKWebViewConfiguration()
  config.mediaTypesRequiringUserActionForPlayback=[]
  config.userContentController.add(self,name:"result")
  web=WKWebView(frame:NSRect(x:0,y:0,width:640,height:400),configuration:config)
  web.navigationDelegate=self;web.uiDelegate=self
  window=NSWindow(contentRect:web.frame,styleMask:[.titled,.closable],backing:.buffered,defer:false)
  window.title="EdenVoice Capture Probe R3 · 真实本机来源";window.contentView=web
  var html = capture ? """
  <!doctype html><meta charset="utf-8"><title>EdenVoice capture test</title>
  <style>body{font:14px system-ui;padding:20px;background:#20212a;color:white}button{padding:10px;margin:6px}video{width:100%;height:180px}pre{white-space:pre-wrap}</style>
  <h2>EdenVoice · 本机采集测试 R3</h2><p>仅本机预览，选择测试窗口即可。不会上传画面。</p>
  <button onclick="start('monitor')">选择桌面</button><button onclick="start('window')">选择窗口</button><button onclick="stop()">停止采集</button>
  <video autoplay muted playsinline></video><pre id="status">等待选择</pre>
  <script>
  const capabilities=()=>({probe:'R3-loopback',secure:isSecureContext,origin:location.origin,url:location.href,mediaDevices:typeof navigator.mediaDevices,display:typeof navigator.mediaDevices?.getDisplayMedia});
  let stream;function report(v){document.querySelector('#status').textContent=JSON.stringify(v,null,2);webkit.messageHandlers.result.postMessage(JSON.stringify(v));}
  function stop(){if(stream){stream.getTracks().forEach(t=>t.stop());stream=null;}document.querySelector('video').srcObject=null;report({stopped:true});}
  async function start(source){if(stream)stop();try{
  if(!navigator.mediaDevices?.getDisplayMedia){report({...capabilities(),error:'CaptureUnavailable',message:'此 WebView 未提供屏幕采集接口'});return;}
  const devices=navigator.mediaDevices;
  stream=await devices.getDisplayMedia({video:{displaySurface:source,width:{ideal:1280,max:1280},height:{ideal:720,max:720},frameRate:{ideal:30,max:30}},audio:true});
  const video=stream.getVideoTracks()[0];await video.applyConstraints({width:{max:1280},height:{max:720},frameRate:{max:30}});
  document.querySelector('video').srcObject=stream;report({source,tracks:stream.getTracks().map(t=>({kind:t.kind,settings:t.getSettings()}))});video.onended=stop;
  }catch(e){stop();report({error:e.name,message:e.message});}}
  window.addEventListener('pagehide',stop);
  report(capabilities());
  </script>
  """ : "<!doctype html><title>Capture capability probe</title>"
  if let index=CommandLine.arguments.firstIndex(of:"--test-html"),index+1<CommandLine.arguments.count {
   do {html=try String(contentsOfFile:CommandLine.arguments[index+1],encoding:.utf8)}
   catch {print(error);NSApp.terminate(nil);return}
  }
  // A baseURL on loadHTMLString is not proof of the document's real origin.
  // Serve an actual loopback document, matching the client's development origin.
  do {
   let parameters=NWParameters.tcp
   parameters.requiredLocalEndpoint = .hostPort(host:"127.0.0.1",port:.any)
   let server=try NWListener(using:parameters);listener=server
   server.newConnectionHandler={connection in
    connection.start(queue:.main)
    connection.receive(minimumIncompleteLength:1,maximumLength:8192){_,_,_,error in
     guard error == nil else {connection.cancel();return}
     let body=Data(html.utf8)
     var response=Data("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: \(body.count)\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n".utf8)
     response.append(body)
     connection.send(content:response,completion:.contentProcessed{_ in connection.cancel()})
    }
   }
   server.stateUpdateHandler={state in
    if case .ready=state,let port=server.port {
     self.web.load(URLRequest(url:URL(string:"http://127.0.0.1:\(port.rawValue)/")!))
    }
    if case .failed(let error)=state {print("probe server failed: \(error)");NSApp.terminate(nil)}
   }
   server.start(queue:.main)
  } catch {print("probe server failed: \(error)");NSApp.terminate(nil)}
  if capture {NSApp.setActivationPolicy(.regular);window.center();window.makeKeyAndOrderFront(nil);NSApp.activate(ignoringOtherApps:true)}
 }
 func userContentController(_ controller:WKUserContentController,didReceive message:WKScriptMessage){
  print(message.body)
  if CommandLine.arguments.contains("--test-html") {
   if let result=message.body as? String,let index=CommandLine.arguments.firstIndex(of:"--report"),index+1<CommandLine.arguments.count {
    try? result.write(toFile:CommandLine.arguments[index+1],atomically:true,encoding:.utf8)
   }
   NSApp.terminate(nil)
  }
 }
 func webView(_ webView:WKWebView,didFinish navigation:WKNavigation!){
  if capture || CommandLine.arguments.contains("--test-html") {return}
  webView.evaluateJavaScript("JSON.stringify({secure:isSecureContext,origin:location.origin,mediaDevices:typeof navigator.mediaDevices,display:typeof navigator.mediaDevices?.getDisplayMedia,peer:typeof RTCPeerConnection,codecs:typeof RTCRtpSender !== 'undefined' ? RTCRtpSender.getCapabilities('video') : null,userAgent:navigator.userAgent})"){value,error in
   let result=value as? String ?? String(describing:error)
   print(result)
   if let index=CommandLine.arguments.firstIndex(of:"--report"),index+1<CommandLine.arguments.count {
    try? result.write(toFile:CommandLine.arguments[index+1],atomically:true,encoding:.utf8)
   }
   NSApp.terminate(nil)
  }
 }
 func webView(_ webView:WKWebView,requestMediaCapturePermissionFor origin:WKSecurityOrigin,initiatedByFrame frame:WKFrameInfo,type:WKMediaCaptureType,decisionHandler:@escaping(WKPermissionDecision)->Void){decisionHandler(.grant)}
}
let app=NSApplication.shared
let probe=Probe();probe.run()
DispatchQueue.main.asyncAfter(deadline:.now()+(probe.capture ? 180 : 15)){print("probe timeout");NSApp.terminate(nil)}
app.run()
