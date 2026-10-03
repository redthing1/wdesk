"use strict";
const token = new URLSearchParams(location.hash.slice(1)).get("token");
history.replaceState(null, "", location.pathname);
const canvas = document.querySelector("canvas"), ctx = canvas.getContext("2d");
const status = document.querySelector("#status");
let observation, pending = Promise.resolve(), stopped = false;
async function api(path, body) {
  const response = await fetch(path, {method: body ? "POST" : "GET", headers: {Authorization: `Bearer ${token}`, ...(body ? {"Content-Type": "application/json"} : {})}, ...(body ? {body: JSON.stringify(body)} : {})});
  if (!response.ok) throw new Error((await response.text()).slice(0, 200));
  return response;
}
async function observe() {
  while (!stopped) {
    try {
      const response = await api("/v1/see");
      observation = JSON.parse(response.headers.get("x-wdesk-observation"));
      const bitmap = await createImageBitmap(await response.blob());
      canvas.width = bitmap.width; canvas.height = bitmap.height;
      ctx.drawImage(bitmap, 0, 0); bitmap.close();
      status.textContent = `${canvas.width} × ${canvas.height} · input ${observation.input_generation}`;
    } catch (e) {status.textContent = `Reconnecting · ${e.message}`;}
    await new Promise(resolve => setTimeout(resolve, 200));
  }
}
function send(actions) {
  pending = pending.then(async () => {
    if (!observation) return;
    const response = await api("/v1/batch", {protocol:1,request_id:crypto.randomUUID(),epoch:observation.epoch,expected_input_generation:null,actions});
    const result = await response.json();
    if (result.results.some(x => !x.delivered)) throw new Error("Input could not be delivered");
  }).catch(e => {status.textContent = e.message;});
}
function point(event) {
  const r=canvas.getBoundingClientRect();
  return {x:Math.min(canvas.width-1, Math.max(0, Math.floor((event.clientX-r.left)*canvas.width/r.width))),y:Math.min(canvas.height-1,Math.max(0,Math.floor((event.clientY-r.top)*canvas.height/r.height)))};
}
let start;
canvas.addEventListener("pointerdown", e => {e.preventDefault();canvas.focus();start={...point(e),button:e.button};canvas.setPointerCapture(e.pointerId);});
canvas.addEventListener("pointerup", e => {
  if (!start) return;
  const end=point(e), button=["left","middle","right"][start.button];
  if (button === "left" && Math.abs(end.x-start.x)+Math.abs(end.y-start.y)>3) send([{type:"drag",x:start.x,y:start.y,to_x:end.x,to_y:end.y}]);
  else if (button) send([{type:"click",...end,button}]);
  start=null;
});
canvas.addEventListener("pointercancel",()=>{start=null;});
canvas.addEventListener("contextmenu",e=>e.preventDefault());
canvas.addEventListener("wheel",e=>{e.preventDefault();send([{type:"move",...point(e)},{type:"scroll",direction:e.deltaY<0?"up":"down",steps:Math.min(10,Math.max(1,Math.ceil(Math.abs(e.deltaY)/100)))}]);},{passive:false});
canvas.addEventListener("keydown",e=>{
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "v") return;
  if (e.key === "Shift" || e.key === "Control" || e.key === "Alt" || e.key === "Meta" || e.isComposing) return;
  e.preventDefault();
  if ([...e.key].length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) send([{type:e.key.charCodeAt(0)>=32&&e.key.charCodeAt(0)<=126?"type_ascii":"type_text",text:e.key}]);
  else {
    const keys=[];
    if(e.ctrlKey)keys.push("CTRL");if(e.altKey)keys.push("ALT");if(e.shiftKey)keys.push("SHIFT");if(e.metaKey)keys.push("WIN");
    keys.push(e.key===" "?"SPACE":e.key);send([{type:"key",keys}]);
  }
});
canvas.addEventListener("paste",e=>{e.preventDefault();send([{type:"type_text",text:e.clipboardData.getData("text/plain")}]);});
canvas.addEventListener("compositionend",e=>{if(e.data)send([{type:"type_text",text:e.data}]);});
document.querySelector("#cad").addEventListener("click",()=>send([{type:"key",keys:["CTRL","ALT","DELETE"]}]));
if (!token) status.textContent="Open this viewer using wdesk view"; else observe();
window.addEventListener("pagehide",()=>{stopped=true;});
