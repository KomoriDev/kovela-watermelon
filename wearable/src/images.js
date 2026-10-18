import { decodeBase64, encodeBase64 } from "./codec.js";

export const IMAGE_SIZES = [36, 46, 58, 72, 89, 108, 128, 149, 173, 192, 192];
export const IMAGE_MAX_BYTES = 40960;
export const IMAGE_CHUNK_BYTES = 1536;
const INDEX = "internal://files/watermelon-images.json";
const PREFIX = "internal://files/watermelon-image-";
const crcTable = [];
for (let n = 0; n < 256; n += 1) {
  let c = n;
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  crcTable.push(c >>> 0);
}
function crcUpdate(crc, bytes) {
  for (let i = 0; i < bytes.length; i += 1) crc = crcTable[(crc ^ bytes[i]) & 255] ^ (crc >>> 8);
  return crc >>> 0;
}
function crcHex(crc) {
  return ("00000000" + ((crc ^ 0xffffffff) >>> 0).toString(16)).slice(-8);
}
function validId(id) {
  return typeof id === "string" && /^[A-Za-z0-9_-]{16,80}$/.test(id);
}
function validLevel(level) {
  return typeof level === "number" && level >= 0 && level < IMAGE_SIZES.length && Math.floor(level) === level;
}
export function validImageHeader(bytes, width, height, length) {
  return bytes.length >= 12 && bytes[0] === 0x19 && bytes[1] === 0x0a &&
    bytes[2] === 0 && bytes[3] === 0 && (bytes[4] & 255) + (bytes[5] & 255) * 256 === width &&
    (bytes[6] & 255) + (bytes[7] & 255) * 256 === height && (bytes[8] & 255) + (bytes[9] & 255) * 256 === width &&
    bytes[10] === 0 && bytes[11] === 0 && length === 12 + 1024 + width * height;
}

const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];
// 小米手环 9/9 Pro 不支持 LVGL v9 .bin，这类设备接收调色板 PNG。PNG 最少要有签名和 IHDR。
export function validPngHeader(bytes, width, height, length) {
  if (bytes.length < 24 || !(length >= 33 && length <= IMAGE_MAX_BYTES)) return false;
  for (let i = 0; i < 8; i += 1) if ((bytes[i] & 255) !== PNG_SIGNATURE[i]) return false;
  const word = offset => ((bytes[offset] & 255) * 16777216) + ((bytes[offset + 1] & 255) << 16) + ((bytes[offset + 2] & 255) << 8) + (bytes[offset + 3] & 255);
  return (bytes[12] & 255) === 73 && (bytes[13] & 255) === 72 && (bytes[14] & 255) === 68 && (bytes[15] & 255) === 82 &&
    word(16) === width && word(20) === height;
}
function formatOf(value) {
  return value && value.format === "png" ? "png" : "bin";
}
function validLength(format, width, height, length) {
  return format === "png" ? length >= 33 && length <= IMAGE_MAX_BYTES : length === 1036 + width * height;
}
function validHeader(format, bytes, width, height, length) {
  return format === "png" ? validPngHeader(bytes, width, height, length) : validImageHeader(bytes, width, height, length);
}
function imageUri(id, level, format) {
  return PREFIX + id + "-" + level + (format === "png" ? ".png" : ".bin");
}

export function createImageStore({ file, loadTimeout }) {
  let entries = {};
  // 设备能显示的格式由游戏探测后设置；不支持 .bin 的设备上旧的 .bin 自定义图不再使用。
  let format = "bin";
  function usable(entry) { return !!entry && (format === "bin" || formatOf(entry) === "png"); }
  function usableLevels() { return Object.keys(entries).map(Number).filter(level => usable(entries[level])); }
  let ready = false;
  let loading = false;
  let loadGeneration = 0;
  let loadTimer = null;
  let pendingEntries = {}
  let transfer = null;
  let busy = false;
  let error = "";
  let cancelPending = false;
  let recovered = false;
  let completed = null;
  let changedLevel = -1;
  let preview = null;
  let previewGeneration = 0;
  const listeners = [];
  const loadWaiters = [];
  function snapshot() {
    const levels = usableLevels();
    return {ready, busy: busy || !!transfer, count: levels.length,
      levels, format, level: transfer ? transfer.level : changedLevel,
      progress: transfer ? Math.floor(transfer.offset * 100 / transfer.length) : 0, error};
  }
  function publish() {
    const state = snapshot();
    for (let i = 0; i < listeners.length; i += 1) listeners[i](state);
  }
  function remove(uri, done) {
    try { file.delete({uri, success: finish, fail: finish}); } catch (_) { finish(); }
    function finish() { if (done) done(); }
  }
  function release() {
    busy = false;
    if (cancelPending) { cancelPending = false; cancel(); }
    if (loadWaiters.length && !busy && !loading && !transfer) load();
  }
  function recoverFiles(generation, done) {
    const keep = {};
    for (const level in entries) keep[entries[level].uri] = true;
    const directories = ["internal://cache/", "internal://files/"];
    let directory = 0;
    function nextDirectory() {
      if (generation !== loadGeneration) return;
      if (directory === directories.length) { done(); return; }
      const uri = directories[directory++];
      try {
        file.list({uri, success(result) {
          if (generation !== loadGeneration) return;
          const list = result.fileList || [];
          let index = 0;
          function nextFile() {
            if (generation !== loadGeneration) return;
            while (index < list.length) {
              const path = list[index++].uri;
              const staging = /^internal:\/\/cache\/watermelon-(?:image-[A-Za-z0-9_-]{16,80}\.part|index-[A-Za-z0-9_-]{16,80}\.json)$/.test(path);
              const orphan = /^internal:\/\/files\/watermelon-image-[A-Za-z0-9_-]{16,80}-\d+\.(?:bin|png)$/.test(path) && !keep[path];
              if (!staging && !orphan) continue;
              remove(path, nextFile);
              return;
            }
            nextDirectory();
          }
          nextFile();
        }, fail: nextDirectory});
      } catch (_) { nextDirectory(); }
    }
    nextDirectory();
  }
  function stopLoadTimer() {
    if (loadTimer) { clearTimeout(loadTimer); loadTimer = null; }
  }
  function finishLoad(generation) {
    if (generation !== loadGeneration) return;
    // 先提交已校验条目：孤立文件回收与 source() 都以此为准，否则刚安装的
    // 图片会被当成孤儿删掉。
    entries = pendingEntries;
    if (!recovered) {
      recoverFiles(generation, function () {
        if (generation !== loadGeneration) return;
        recovered = true;
        finishLoad(generation);
      });
      return;
    }
    stopLoadTimer();
    ready = true;
    loading = false;
    publish();
    const waiters = loadWaiters.splice(0);
    for (let i = 0; i < waiters.length; i += 1) waiters[i]();
  }
  function load(done) {
    if (typeof done === "function") loadWaiters.push(done);
    if (loading || busy || transfer) return;
    beginLoad(false);
  }
  function beginLoad(retrying) {
    loading = true;
    const generation = ++loadGeneration;
    clearPreview();
    // 真机上文件回调可能丢失（切后台、刚启动时最明显）。加载看门狗保证
    // loading 不会永久占住：先自动重读一次，仍无回调才按已校验的条目出
    // 结论。否则预览只会一直收到 NOT_READY，插件的分块重读也会耗尽轮次
    // 后报「读取图片超时」，游戏页则永远停在原生图片。
    stopLoadTimer();
    loadTimer = setTimeout(function () {
      if (generation !== loadGeneration || !loading) return;
      if (!retrying) { beginLoad(true); return; }
      recovered = true;
      finishLoad(generation);
    }, loadTimeout || 4000);
    // 重读时保留上一轮已校验的条目：两轮都丢回调也能按已有结果出结论。
    if (!retrying) pendingEntries = {};
    file.readText({uri: INDEX, success(result) {
      if (generation !== loadGeneration) return;
      let stored = {};
      try {
        if (result.text.length > 4096) throw new Error("index");
        const value = JSON.parse(result.text);
        if (value.v !== 1 || !value.images || typeof value.images !== "object") throw new Error("index");
        stored = value.images;
      } catch (_) { stored = {}; }
      let level = 0;
      function readNext() {
        if (generation !== loadGeneration) return;
        if (level === IMAGE_SIZES.length) { finishLoad(generation); return; }
        const slot = level++;
        const entry = stored[slot];
        if (!entry || !validId(entry.id) || entry.uri !== imageUri(entry.id, slot, formatOf(entry)) ||
            entry.width !== entry.height || !(entry.width > 0 && entry.width <= IMAGE_SIZES[slot]) ||
            !validLength(formatOf(entry), entry.width, entry.height, entry.length)) { readNext(); return; }
        file.get({uri: entry.uri, success(info) {
          if (generation !== loadGeneration) return;
          if (info.length !== entry.length) { readNext(); return; }
          file.readArrayBuffer({uri: entry.uri, length: 24, success(data) {
            if (generation !== loadGeneration) return;
            if (validHeader(formatOf(entry), data.buffer, entry.width, entry.height, entry.length)) pendingEntries[slot] = entry;
            readNext();
          }, fail: readNext});
        }, fail: readNext});
      }
      readNext();
    }, fail() {
      if (generation !== loadGeneration) return;
      pendingEntries = {}; finishLoad(generation);
    }});
  }
  function reply(send, value, stage, success, extra) {
    send(Object.assign({v: 1, id: value.id, type: "image-result", stage, success}, extra || {}));
  }
  function reject(send, value, stage, code) {
    error = code;
    publish();
    reply(send, value, stage, false, {error: code});
  }
  function saveIndex(next, id, done) {
    const staging = "internal://cache/watermelon-index-" + id + ".json";
    file.writeText({uri: staging, text: JSON.stringify({v: 1, images: next}), success() {
      file.move({srcUri: staging, dstUri: INDEX, success() { entries = next; done(true); }, fail() { remove(staging, function () { done(false); }); }});
    }, fail() { remove(staging, function () { done(false); }); }});
  }
  function clearPreview() {
    previewGeneration += 1;
    preview = null;
  }
  function readPreview(value, send) {
    if (!validId(value.id) || !validLevel(value.level)) return;
    const base = {v: 1, id: value.id, type: "image-preview", level: value.level};
    function fail(code) { send(Object.assign({}, base, {success: false, error: code})); }
    if (!ready || loading) { fail("NOT_READY"); return; }
    if (busy || transfer) { fail("BUSY"); return; }
    const entry = entries[value.level];
    if (!usable(entry)) { send(Object.assign(base, {success: true, default: true})); return; }
    const kind = formatOf(entry);
    if (typeof value.offset !== "number" || Math.floor(value.offset) !== value.offset ||
        value.offset < 0 || value.offset >= entry.length || value.offset % IMAGE_CHUNK_BYTES !== 0) {
      fail("OFFSET_MISMATCH"); return;
    }
    function deliver(bytes) {
      const chunk = bytes.subarray(value.offset, Math.min(entry.length, value.offset + IMAGE_CHUNK_BYTES));
      send(Object.assign(base, {success: true, format: kind, width: entry.width, length: entry.length,
        offset: value.offset, data: encodeBase64(chunk)}));
    }
    // 上一次读取如果在应用切后台时丢了文件回调，缓存会一直停在 bytes=null。
    // 这类重发不能静默丢弃：直接重新发起读取（generation 自增让旧回调作废），
    // 手环回到前台后再重发一次即可恢复，不用等插件换 id 整段重读。
    if (preview && preview.id === value.id && preview.uri === entry.uri && preview.bytes) {
      deliver(preview.bytes);
      return;
    }
    if (value.offset !== 0) { fail("NO_PREVIEW"); return; }
    clearPreview();
    const generation = previewGeneration;
    const current = preview = {id: value.id, uri: entry.uri, bytes: null};
    try {
      // 必须带 length：真机上不带长度的读取不会回调（其他调用点都显式传长度），
      // 于是请求进来后既无响应也无错误，插件只能等到超时。
      file.readArrayBuffer({uri: entry.uri, length: entry.length, success(result) {
        if (generation !== previewGeneration || preview !== current) return;
        const bytes = result.buffer instanceof Uint8Array ? result.buffer : new Uint8Array(result.buffer);
        if (bytes.length !== entry.length || !validHeader(kind, bytes, entry.width, entry.height, entry.length)) {
          clearPreview(); fail("INVALID_IMAGE"); return;
        }
        current.bytes = bytes;
        deliver(bytes);
      }, fail() { if (preview === current) { clearPreview(); fail("READ_FAILED"); } }});
    } catch (_) { clearPreview(); fail("READ_FAILED"); }
  }
  function receive(value, send) {
    if (value.type === "image-read") { readPreview(value, send); return true; }
    const stages = {"image-begin": "begin", "image-chunk": "chunk", "image-commit": "commit", "image-reset": "reset"};
    const stage = stages[value.type];
    if (!stage) return false;
    if (!validId(value.id) || !validLevel(value.level)) return true;
    if (!ready || loading) { reject(send, value, stage, "NOT_READY"); return true; }
    if (completed && completed.id === value.id && completed.level === value.level && completed.stage === stage) {
      reply(send, value, stage, true); return true;
    }
    if (busy) return true;
    error = "";
    if (stage === "begin") {
      const kind = formatOf(value);
      // 不支持 .bin 的设备只收 PNG：旧插件发来的 .bin 装上也显示不出来。
      if (format === "png" && kind !== "png") { reject(send, value, stage, "UNSUPPORTED_FORMAT"); return true; }
      if (transfer && transfer.id === value.id && transfer.level === value.level) {
        if (transfer.format !== kind || transfer.length !== value.length || transfer.crc32 !== value.crc32 || transfer.width !== value.width || transfer.height !== value.height) {
          reject(send, value, stage, "INVALID_IMAGE");
        } else reply(send, value, stage, true, {offset: transfer.offset});
        return true;
      }
      if (typeof value.width !== "number" || Math.floor(value.width) !== value.width || value.width <= 0 ||
          value.width > IMAGE_SIZES[value.level] || value.height !== value.width ||
          !validLength(kind, value.width, value.height, value.length) || value.length > IMAGE_MAX_BYTES ||
          typeof value.crc32 !== "string" || !/^[0-9a-f]{8}$/.test(value.crc32)) {
        reject(send, value, stage, "INVALID_IMAGE"); return true;
      }
      busy = true;
      clearPreview();
      const previous = transfer;
      transfer = {id: value.id, level: value.level, format: kind, width: value.width, height: value.height, length: value.length,
        crc32: value.crc32, crc: 0xffffffff, offset: 0, uri: "internal://cache/watermelon-image-" + value.id + ".part"};
      const current = transfer;
      function begin() {
        file.writeArrayBuffer({uri: current.uri, buffer: new Uint8Array(0), success() {
          release();
          if (transfer !== current) return;
          publish();
          reply(send, value, stage, true, {offset: 0});
        }, fail() { transfer = null; release(); reject(send, value, stage, "WRITE_FAILED"); }});
      }
      if (previous) remove(previous.uri, begin); else begin();
      publish();
      return true;
    }
    if (stage === "reset") {
      clearPreview();
      if (transfer) { reject(send, value, stage, "BUSY"); return true; }
      const old = entries[value.level];
      if (!old) { changedLevel = value.level; publish(); reply(send, value, stage, true); return true; }
      busy = true;
      const next = Object.assign({}, entries);
      delete next[value.level];
      saveIndex(next, value.id, function (success) {
        release();
        if (!success) { reject(send, value, stage, "WRITE_FAILED"); return; }
        completed = {id: value.id, level: value.level, stage};
        changedLevel = value.level;
        remove(old.uri);
        publish();
        reply(send, value, stage, true);
      });
      publish();
      return true;
    }
    const current = transfer;
    if (!current || current.id !== value.id || current.level !== value.level) {
      reject(send, value, stage, "NO_TRANSFER"); return true;
    }
    if (stage === "chunk") {
      let bytes;
      try {
        if (typeof value.data !== "string" || value.data.length > 2048) throw new Error("chunk");
        bytes = decodeBase64(value.data);
      } catch (_) { reject(send, value, stage, "INVALID_CHUNK"); return true; }
      if (!bytes.length || bytes.length > IMAGE_CHUNK_BYTES || typeof value.offset !== "number" ||
          Math.floor(value.offset) !== value.offset || value.offset < 0 || value.offset + bytes.length > current.length) {
        reject(send, value, stage, "INVALID_CHUNK"); return true;
      }
      if (value.offset < current.offset) { reply(send, value, stage, true, {offset: current.offset}); return true; }
      if (value.offset !== current.offset) { reject(send, value, stage, "OFFSET_MISMATCH"); return true; }
      if (current.offset === 0 && !validHeader(current.format, bytes, current.width, current.height, current.length)) {
        reject(send, value, stage, "INVALID_IMAGE"); return true;
      }
      busy = true;
      file.writeArrayBuffer({uri: current.uri, buffer: bytes, append: true, success() {
        current.crc = crcUpdate(current.crc, bytes);
        current.offset += bytes.length;
        release();
        if (transfer !== current) return;
        publish();
        reply(send, value, stage, true, {offset: current.offset});
      }, fail() { release(); reject(send, value, stage, "WRITE_FAILED"); }});
      return true;
    }
    if (current.offset !== current.length || crcHex(current.crc) !== current.crc32) {
      reject(send, value, stage, "CHECKSUM_MISMATCH"); return true;
    }
    busy = true;
    const destination = imageUri(current.id, current.level, current.format);
    const old = entries[current.level];
    file.move({srcUri: current.uri, dstUri: destination, success() {
      const next = Object.assign({}, entries);
      next[current.level] = {id: current.id, uri: destination, width: current.width, height: current.height, length: current.length};
      if (current.format === "png") next[current.level].format = "png";
      saveIndex(next, current.id, function (success) {
        transfer = null;
        if (!success) {
          file.move({srcUri: destination, dstUri: current.uri, success() { transfer = current; release(); reject(send, value, stage, "WRITE_FAILED"); },
            fail() { remove(destination, function () { release(); reject(send, value, stage, "WRITE_FAILED"); }); }});
          return;
        }
        completed = {id: current.id, level: current.level, stage};
        clearPreview();
        changedLevel = current.level;
        release();
        if (old) remove(old.uri);
        publish();
        reply(send, value, stage, true);
      });
    }, fail() { release(); reject(send, value, stage, "WRITE_FAILED"); }});
    publish();
    return true;
  }
  function cancel() {
    clearPreview();
    // 退出页面可能丢弃原生文件回调；旧加载不能阻塞下一次进入。
    stopLoadTimer();
    loadGeneration += 1;
    if (loading) {
      loading = false;
      loadWaiters.splice(0);
    }
    if (busy) { cancelPending = true; return; }
    const current = transfer;
    transfer = null;
    if (current) {
      busy = true;
      remove(current.uri, function () { release(); publish(); });
    } else {
      publish();
      if (loadWaiters.length && !loading) load();
    }
  }
  return {
    load, receive, cancel, snapshot,
    format() { return format; },
    setFormat(value) {
      const next = value === "png" ? "png" : "bin";
      if (next === format) return;
      format = next;
      publish();
    },
    source(level) { return usable(entries[level]) ? entries[level].uri : "/common/fruit-" + (level + 1) + ".bin"; },
    subscribe(listener) { listeners.push(listener); listener(snapshot()); return function () { const i = listeners.indexOf(listener); if (i >= 0) listeners.splice(i, 1); }; },
  };
}
