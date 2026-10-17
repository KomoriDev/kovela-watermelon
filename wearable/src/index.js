import {createImageStore, IMAGE_SIZES, IMAGE_MAX_BYTES, IMAGE_CHUNK_BYTES} from "./images.js";

const PRODUCT = "com.komoridev.watermelon";
export function createSkins({file, interconnect}) {
  const images = createImageStore({file});
  const listeners = [];
  let connection = null, connected = false, error = "", generation = 0;
  function snapshot() { return {ready: images.snapshot().ready, connected, error}; }
  function publish() { const state = snapshot(); for (let i=0;i<listeners.length;i++) listeners[i](state); }
  images.subscribe(publish);
  function send(value) {
    if (!connection) return;
    try { connection.send({data: value, success() {}, fail() { connected=false; error="传输失败，请检查 AstroBox 连接"; publish(); }}); }
    catch (_) { connected=false; error="传输失败，请检查 AstroBox 连接"; publish(); }
  }
  function receive(event) {
    let value = event && event.data;
    try {
      if (typeof value === "string") { if (value.length>4096) return; value=JSON.parse(value); }
      if (!value || value.v!==1 || typeof value.id!=="string" || !/^[A-Za-z0-9_-]{16,80}$/.test(value.id)) return;
      connected=true; error=""; publish();
      if (value.type==="hello") {
        const current=generation;
        function reply() {
          if(current!==generation) return;
          send({v:1,id:value.id,type:"device",productId:PRODUCT,
            images:{v:1,freeTransfer:true,sizes:IMAGE_SIZES,maxBytes:IMAGE_MAX_BYTES,chunkBytes:IMAGE_CHUNK_BYTES,
              customLevels:images.snapshot().levels,preview:true}});
        }
        if(images.snapshot().ready) reply(); else images.load(reply);
      } else images.receive(value,send);
    } catch (_) { return; }
  }
  function retry() {
    error="";
    if(!connection) {
      try {
        connection=interconnect.instance();
        connection.onmessage=receive;
        connection.onopen=function(){connected=true;publish();};
        connection.onclose=function(){connected=false;publish();};
        connection.onerror=function(){connected=false;error="请连接 AstroBox 并打开合成大西瓜换肤插件";publish();};
      } catch (_) { error="设备通信不可用，请检查 AstroBox 连接"; publish(); return; }
    }
    // Avoid unbounded getReadyState: page callbacks and valid messages report connectivity.
    if(typeof connection.diagnosis==="function") {
      try { connection.diagnosis({timeout:1500,success(info){connected=!!(info&&info.status===0);publish();},fail(){connected=false;publish();}}); } catch (_) {}
    }
    images.load();publish();
  }
  function resetImage(level,done) {
    if(images.snapshot().busy || typeof level!=="number" || level%1!==0 || level<0 || level>=11) { if(done)done(false);return; }
    const id="reset-image-"+Date.now().toString(36)+"-"+level;
    images.receive({v:1,id,type:"image-reset",level},function(reply){if(done)done(reply.success===true);});
  }
  return {
    images, snapshot, retry, resetImage,
    start() { images.load(); },
    subscribe(listener) { listeners.push(listener);listener(snapshot());return function(){const i=listeners.indexOf(listener);if(i>=0)listeners.splice(i,1);}; },
    dispose() {
      generation++;
      if(connection){connection.onmessage=null;connection.onopen=null;connection.onclose=null;connection.onerror=null;}
      connection=null;connected=false;error="";images.cancel();listeners.splice(0);
    }
  };
}
