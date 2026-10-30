import test from "node:test";
import assert from "node:assert/strict";
import { createImageStore, validPngHeader } from "../src/images.js";
import { crc32 } from "node:zlib";

function environment() {
  const files = new Map();
  let moveFailure = false;
  const file = {
    list({uri, success}) { success({fileList: [...files.keys()].filter(key => key.startsWith(uri)).map(uri => ({uri}))}); },
    readText({uri, success, fail}) { files.has(uri) ? success({text: files.get(uri).toString()}) : fail(); },
    get({uri, success, fail}) { files.has(uri) ? success({length: files.get(uri).length}) : fail(); },
    // 真机上不带 length 的读取不会回调，mock 保持同样约束，避免再写出静默的读取。
    readArrayBuffer({uri, length, success, fail}) {
      if (!files.has(uri) || typeof length !== "number" || !(length > 0) || length > files.get(uri).length) { fail(); return; }
      success({buffer: files.get(uri).subarray(0, length)});
    },
    writeText({uri, text, success}) { files.set(uri, Buffer.from(text)); success(); },
    writeArrayBuffer({uri, buffer, append, success}) { files.set(uri, append ? Buffer.concat([files.get(uri) || Buffer.alloc(0), buffer]) : Buffer.from(buffer)); success(); },
    move({srcUri, dstUri, success, fail}) {
      if (moveFailure && dstUri.endsWith("watermelon-images.json")) { fail(); return; }
      if (!files.has(srcUri)) { fail(); return; }
      files.set(dstUri, files.get(srcUri)); files.delete(srcUri); success();
    },
    delete({uri, success}) { files.delete(uri); success(); },
  };
  function open() { const store = createImageStore({file}); store.load(); return store; }
  const store = open();
  function request(value, target = store) {
    let reply;
    assert.equal(target.receive({v: 1, ...value}, (result) => { reply = result; }), true);
    return reply;
  }
  return {files, file, store, request, open, failCommit(value) { moveFailure = value; }};
}
// 手环只校验 PNG 签名与 IHDR 尺寸；默认 1612 字节，正好需要两块传输。
function png(size = 24, length = 1612) {
  const bytes = Buffer.alloc(length);
  bytes.set([137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82]);
  bytes.writeUInt32BE(size, 16); bytes.writeUInt32BE(size, 20);
  bytes.set([8, 3, 0, 0, 0], 24);
  bytes.fill(1, 33);
  return bytes;
}
function begin(e, bytes, id, crc = crc32(bytes).toString(16).padStart(8, "0"), target) {
  return e.request({type: "image-begin", id, level: 0, format: "png", width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20), length: bytes.length, crc32: crc}, target);
}
function chunks(e, bytes, id) {
  for (let offset = 0; offset < bytes.length; offset += 1536) {
    const chunk = {type: "image-chunk", id, level: 0, offset, data: bytes.subarray(offset, offset + 1536).toString("base64url")};
    const first = e.request(chunk);
    assert.equal(first.success, true);
    assert.deepEqual(e.request(chunk), first);
  }
}
const ID = "image-test-00000001";
const NEXT = "image-test-00000002";

test("duplicate chunks and commit preserve one image through restart; reset removes it", async () => {
  const e = environment(), bytes = png();
  assert.equal(begin(e, bytes, ID).success, true);
  chunks(e, bytes, ID);
  assert.equal(e.store.source(0), "/common/fruit-1.png");
  const commit = {type: "image-commit", id: ID, level: 0};
  assert.equal(e.request(commit).success, true);
  assert.equal(e.request(commit).success, true);
  const source = e.store.source(0);
  assert.deepEqual(e.files.get(source), bytes);
  const reboot = e.open();
  assert.equal(reboot.source(0), source);
  assert.equal(e.request({type: "image-reset", id: NEXT, level: 0}, reboot).success, true);
  assert.equal(reboot.source(0), "/common/fruit-1.png");
  assert.equal(e.files.has(source), false);
});

test("CRC mismatch and interrupted upload cannot replace current image", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const old = e.store.source(0);
  begin(e, bytes, NEXT, "00000000"); chunks(e, bytes, NEXT);
  const result = e.request({type: "image-commit", id: NEXT, level: 0});
  assert.equal(result.error, "CHECKSUM_MISMATCH");
  assert.equal(e.store.source(0), old);
  e.store.cancel();
  assert.equal(e.open().source(0), old);
});

test("index commit failure leaves old image and supports commit retry", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const old = e.store.source(0);
  begin(e, bytes, NEXT); chunks(e, bytes, NEXT);
  e.failCommit(true);
  assert.equal(e.request({type: "image-commit", id: NEXT, level: 0}).error, "WRITE_FAILED");
  assert.equal(e.store.source(0), old);
  e.failCommit(false);
  assert.equal(e.request({type: "image-commit", id: NEXT, level: 0}).success, true);
  assert.notEqual(e.store.source(0), old);
  assert.equal(e.files.has(old), false);
});

test("oversized and invalid-header uploads are rejected before installation", async () => {
  const e = environment(), bytes = png();
  assert.equal(e.request({type: "image-begin", id: ID, level: 0, format: "png", width: 192, height: 192, length: 37900, crc32: "12345678"}).error, "INVALID_IMAGE");
  begin(e, bytes, ID);
  const broken = Buffer.from(bytes); broken[0] = 0;
  assert.equal(e.request({type: "image-chunk", id: ID, level: 0, offset: 0, data: broken.subarray(0,1536).toString("base64url")}).error, "INVALID_IMAGE");
});

test("restart discards staging and orphan files without removing installed images or unrelated files", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const old = e.store.source(0);
  begin(e, bytes, NEXT);
  const part = "internal://cache/watermelon-image-" + NEXT + ".part";
  const staging = "internal://cache/watermelon-index-" + NEXT + ".json";
  const orphan = "internal://files/watermelon-image-" + NEXT + "-0.png";
  const legacy = "internal://files/watermelon-image-" + NEXT + "-1.bin";
  const unrelated = "internal://files/user-picture.bin";
  for (const uri of [staging, orphan, legacy, unrelated]) e.files.set(uri, bytes);
  const reboot = e.open();
  assert.equal(reboot.source(0), old);
  assert.deepEqual(e.files.get(old), bytes);
  for (const uri of [part, staging, orphan, legacy]) assert.equal(e.files.has(uri), false);
  assert.deepEqual(e.files.get(unrelated), bytes);
  assert.equal(begin(e, bytes, NEXT, undefined, reboot).success, true);
});

test("leaving during a chunk write cancels staging after its callback and releases queued reload", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const old = e.store.source(0);
  begin(e, bytes, NEXT);
  const write = e.file.writeArrayBuffer;
  let pending;
  e.file.writeArrayBuffer = args => { pending = () => write(args); };
  e.request({type: "image-chunk", id: NEXT, level: 0, offset: 0, data: bytes.subarray(0, 1536).toString("base64url")});
  e.store.cancel();
  let loaded = false;
  e.store.load(() => { loaded = true; });
  assert.equal(loaded, false);
  pending();
  assert.equal(loaded, true);
  assert.equal(e.store.snapshot().busy, false);
  assert.equal(e.store.source(0), old);
  assert.equal(e.files.has("internal://cache/watermelon-image-" + NEXT + ".part"), false);
});

for (const operation of ["readText", "get", "readArrayBuffer", "list"]) {
  test("exit during " + operation + " reload abandons stale callbacks and permits reentry", async () => {
    const e = environment(), bytes = png();
    begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
    const original = e.file[operation];
    const abandoned = [];
    e.file[operation] = args => { abandoned.push(args); };
    const store = operation === "list" ? e.open() : e.store;
    let oldLoaded = false;
    store.load(() => { oldLoaded = true; });
    assert.ok(abandoned.length > 0);
    store.cancel();
    e.file[operation] = original;
    let loaded = false;
    store.load(() => { loaded = true; });
    assert.equal(loaded, true, "新页面必须能独立完成读取，不能等待退出页面的旧回调");
    assert.deepEqual(store.snapshot().levels, [0]);
    const received = [];
    for (let offset = 0; offset < bytes.length; offset += 1536) {
      const reply = e.request({type: "image-read", id: NEXT, level: 0, offset}, store);
      assert.equal(reply.success, true);
      received.push(Buffer.from(reply.data, "base64url"));
    }
    assert.deepEqual(Buffer.concat(received), bytes);
    for (const args of abandoned) original(args);
    assert.equal(oldLoaded, false, "不能调用已退出页面的加载完成回调");
    assert.deepEqual(store.snapshot().levels, [0]);
  });
}

test("cancelled index callbacks cannot overwrite images installed after reentry", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const original = e.file.readText;
  let stale;
  e.file.readText = args => { original({...args, success: result => { stale = () => args.success(result); }}); };
  e.store.load();
  e.store.cancel();
  e.file.readText = original;
  e.request({type: "image-reset", id: "reset-after-exit-01", level: 0});
  e.store.load();
  stale();
  assert.deepEqual(e.store.snapshot().levels, []);
  assert.equal(e.request({type: "image-read", id: NEXT, level: 0, offset: 0}).default, true);
});

test("192px PNG headers remain valid with signed native file bytes", () => {
  const header = Int8Array.from(png(192).subarray(0, 24));
  assert.equal(validPngHeader(header, 192, 192, 20000), true);
  assert.equal(validPngHeader(header, 149, 149, 20000), false);
});

test("preview reads exact current image in bounded chunks and reports default after reset", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID);
  e.request({type: "image-commit", id: ID, level: 0});
  assert.deepEqual(e.store.snapshot().levels, [0]);
  const received = [];
  for (let offset = 0; offset < bytes.length; offset += 1536) {
    const request = {type: "image-read", id: NEXT, level: 0, offset};
    const reply = e.request(request);
    assert.equal(reply.success, true);
    assert.equal(reply.offset, offset);
    const chunk = Buffer.from(reply.data, "base64url");
    assert.equal(chunk.length, Math.min(1536, bytes.length - offset));
    received.push(chunk);
    assert.deepEqual(e.request(request), reply);
  }
  assert.deepEqual(Buffer.concat(received), bytes);
  assert.equal(e.request({type: "image-read", id: NEXT, level: 0, offset: 1}).error, "OFFSET_MISMATCH");
  e.request({type: "image-reset", id: "reset-preview-0001", level: 0});
  assert.equal(e.store.snapshot().level, 0);
  assert.deepEqual(e.store.snapshot().levels, []);
  assert.equal(e.request({type: "image-read", id: NEXT, level: 0, offset: 0}).default, true);
});

test("lost preview callbacks are re-armed by the next request", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const original = e.file.readArrayBuffer;
  const lost = [];
  e.file.readArrayBuffer = args => { lost.push(() => original(args)); };
  let reply;
  e.store.receive({v: 1, type: "image-read", id: NEXT, level: 0, offset: 0}, value => { reply = value; });
  assert.equal(reply, undefined);
  assert.equal(lost.length, 1);
  // 应用切后台丢掉回调后，重发的同一个请求必须重新发起读取，而不是静默丢弃。
  e.store.receive({v: 1, type: "image-read", id: NEXT, level: 0, offset: 0}, value => { reply = value; });
  assert.equal(lost.length, 2);
  lost[1]();
  assert.equal(reply.success, true);
  assert.equal(reply.offset, 0);
  assert.equal(Buffer.from(reply.data, "base64url").length, 1536);
  // 迟到的旧回调不能覆盖新一次读取的结果。
  const settled = reply;
  lost[0]();
  assert.deepEqual(reply, settled);
});

test("preview cannot publish a cancelled stale read and can resume freely", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const original = e.file.readArrayBuffer;
  let deliver;
  e.file.readArrayBuffer = args => { deliver = () => original(args); };
  let reply;
  e.store.receive({v:1,type:"image-read",id:NEXT,level:0,offset:0}, value => { reply = value; });
  e.store.cancel();
  deliver();
  assert.equal(reply, undefined);
  e.file.readArrayBuffer = original;
  const resumed = e.request({type:"image-read",id:NEXT,level:0,offset:0});
  assert.equal(resumed.success, true);
  assert.deepEqual(Buffer.from(resumed.data, "base64url"), bytes.subarray(0,1536));
});

async function until(predicate, timeout = 2000) {
  const deadline = Date.now() + timeout;
  while (!predicate() && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 5));
}

test("a lost index callback retries once and then releases loading instead of stalling previews", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  let calls = 0;
  e.file.readText = () => { calls += 1; };
  const store = createImageStore({file: e.file, loadTimeout: 15});
  let loaded = false;
  store.load(() => { loaded = true; });
  assert.equal(store.snapshot().ready, false);
  await until(() => loaded);
  assert.equal(calls, 2, "丢失回调后先自动重读一次，仍无回调才出结论");
  assert.equal(loaded, true, "看门狗必须给加载出结论，不能永久占用 loading");
  assert.equal(store.snapshot().ready, true);
  assert.deepEqual(store.snapshot().levels, []);
  // loading 已释放：预览请求得到明确回执，插件不会一直等到重读轮次耗尽。
  assert.equal(e.request({type: "image-read", id: NEXT, level: 0, offset: 0}, store).default, true);
});

test("watchdog releases a load stalled on a missing header callback and a later load recovers", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const original = e.file.readArrayBuffer;
  let disabled = true;
  e.file.readArrayBuffer = args => { if (disabled) return; original(args); };
  const store = createImageStore({file: e.file, loadTimeout: 15});
  let loaded = false;
  store.load(() => { loaded = true; });
  await until(() => loaded);
  assert.equal(loaded, true);
  assert.deepEqual(store.snapshot().levels, [], "回调丢失时不能把未校验的图片当自定义图");
  disabled = false;
  let recovered = false;
  store.load(() => { recovered = true; });
  assert.equal(recovered, true);
  assert.deepEqual(store.snapshot().levels, [0]);
  const reply = e.request({type: "image-read", id: NEXT, level: 0, offset: 0}, store);
  assert.equal(reply.success, true);
  assert.equal(Buffer.from(reply.data, "base64url").length, 1536);
});

test("reloads that lose file callbacks keep installed images instead of flipping to built-ins", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const store = createImageStore({file: e.file, loadTimeout: 15});
  store.load();
  const custom = store.source(0);
  assert.match(custom, /watermelon-image-.*-0\.png$/);
  assert.equal(store.snapshot().complete, true);
  const {readText, readArrayBuffer} = e.file;
  // 索引回调两轮都丢失：全部沿用上一轮。
  e.file.readText = () => {};
  let done = false;
  store.load(() => { done = true; });
  await until(() => done);
  assert.equal(store.source(0), custom, "丢回调不能把自定义图换回内置图");
  assert.equal(store.snapshot().complete, false);
  // 索引读到但图片头回调丢失：索引里仍是同一文件，沿用。
  e.file.readText = readText;
  e.file.readArrayBuffer = () => {};
  done = false;
  store.load(() => { done = true; });
  await until(() => done);
  assert.equal(store.source(0), custom);
  assert.deepEqual(e.files.get(custom), bytes, "沿用时不能把文件当孤儿删掉");
  // 回调恢复后重读完整，标记为完整加载。
  e.file.readArrayBuffer = readArrayBuffer;
  store.load();
  assert.equal(store.source(0), custom);
  assert.equal(store.snapshot().complete, true);
});

test("a stalled reload still drops images that another page reset meanwhile", async () => {
  const e = environment(), bytes = png();
  begin(e, bytes, ID); chunks(e, bytes, ID); e.request({type: "image-commit", id: ID, level: 0});
  const game = createImageStore({file: e.file, loadTimeout: 15});
  game.load();
  assert.notEqual(game.source(0), "/common/fruit-1.png");
  // 换肤页（另一份实例）把等级 0 恢复默认。
  assert.equal(e.request({type: "image-reset", id: NEXT, level: 0}).success, true);
  e.file.readArrayBuffer = () => {};
  e.file.get = () => {};
  let done = false;
  game.load(() => { done = true; });
  await until(() => done);
  assert.equal(game.source(0), "/common/fruit-1.png");
});

test("PNG images install, reload and preview", async () => {
  const e = environment(), bytes = png();
  assert.equal(begin(e, bytes, ID).success, true);
  chunks(e, bytes, ID);
  assert.equal(e.request({type: "image-commit", id: ID, level: 0}).success, true);
  const source = e.store.source(0);
  assert.match(source, /^internal:\/\/files\/watermelon-image-image-test-00000001-0\.png$/);
  assert.deepEqual(e.files.get(source), bytes);
  assert.equal(JSON.parse(e.files.get("internal://files/watermelon-images.json")).images[0].format, "png");
  const reboot = e.open();
  assert.equal(reboot.source(0), source);
  const reply = e.request({type: "image-read", id: NEXT, level: 0, offset: 0}, reboot);
  assert.equal(reply.success, true);
  assert.equal(reply.format, "png");
  assert.deepEqual(Buffer.from(reply.data, "base64url"), bytes.subarray(0, 1536));
});

test(".bin uploads are rejected and old .bin images fall back to the built-in PNG", async () => {
  const e = environment();
  const legacy = "internal://files/watermelon-image-" + ID + "-0.bin";
  e.files.set(legacy, Buffer.alloc(1612));
  e.files.set("internal://files/watermelon-images.json", Buffer.from(JSON.stringify({v: 1, images: {0: {id: ID, uri: legacy, width: 24, height: 24, length: 1612}}})));
  const store = e.open();
  assert.equal(store.source(0), "/common/fruit-1.png");
  assert.deepEqual(store.snapshot().levels, []);
  assert.equal(e.request({type: "image-read", id: NEXT, level: 0, offset: 0}, store).default, true);
  assert.equal(e.files.has(legacy), false, "旧 .bin 文件当作孤儿清理");
  const bin = {type: "image-begin", id: NEXT, level: 0, width: 24, height: 24, length: 1612, crc32: "12345678"};
  assert.equal(e.request(bin, store).error, "UNSUPPORTED_FORMAT");
  assert.equal(e.request({...bin, format: "bin"}, store).error, "UNSUPPORTED_FORMAT");
});

test("PNG uploads validate the signature and size limits", async () => {
  const e = environment();
  assert.equal(e.request({type: "image-begin", id: ID, level: 0, format: "png", width: 24, height: 24, length: 40961, crc32: "12345678"}).error, "INVALID_IMAGE");
  assert.equal(e.request({type: "image-begin", id: ID, level: 0, format: "png", width: 24, height: 24, length: 32, crc32: "12345678"}).error, "INVALID_IMAGE");
  const bytes = png();
  begin(e, bytes, ID);
  const broken = Buffer.from(bytes); broken[1] = 0;
  assert.equal(e.request({type: "image-chunk", id: ID, level: 0, offset: 0, data: broken.subarray(0, 1536).toString("base64url")}).error, "INVALID_IMAGE");
  const wrongSize = png(30);
  assert.equal(begin(e, wrongSize, NEXT).success, true);
  assert.equal(e.request({type: "image-chunk", id: NEXT, level: 0, offset: 0, data: png(24).subarray(0, 1536).toString("base64url")}).error, "INVALID_IMAGE");
});