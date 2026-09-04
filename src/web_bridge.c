#include <emscripten.h>
#include <stdint.h>

EM_JS(void, neolove_js_bootstrap, (), {
  if (typeof Module !== "object") {
    globalThis.Module = {};
  }

  const module = Module;
  const state = module.neoloveState || (module.neoloveState = {
    initialized: false,
    canvasWidth: 1,
    canvasHeight: 1,
    mouseX: 0,
    mouseY: 0,
    buttonsDown: [0, 0, 0, 0, 0],
    buttonsPressed: [0, 0, 0, 0, 0],
    buttonsReleased: [0, 0, 0, 0, 0],
    keysDown: Object.create(null),
    keysPressed: Object.create(null),
    keysReleased: Object.create(null),
    wheelX: 0,
    wheelY: 0,
    lastKey: "",
    charPressed: "",
    ctx: null,
    imageData: null,
    overlayEl: null,
    detailEl: null,
    statusEl: null,
    audio: {
      context: null,
      active: new Map(),
      active3d: new Map(),
      lastError: "",
      resumeHooksInstalled: false
    },
    media: {
      queue: [],
      current: null,
      cancelled: new Set(),
      streams: new Map(),
      stoppedErrors: new Map(),
      nextStreamId: 1,
      permissions: { microphone: "prompt", camera: "prompt" },
      permissionQueries: { microphone: false, camera: false },
      cleanupInstalled: false
    },
    fonts: new Map(),
    images: new Map(),
    imageUseCounter: 0
  });

  // Keep hot-reloaded pages created by an older bridge revision compatible.
  if (!state.audio.active3d) {
    state.audio.active3d = new Map();
  }

  const canvas = document.getElementById("canvas");
  if (!canvas) {
    throw new Error("NeoLOVE web runtime requires a <canvas id=\"canvas\"> element");
  }

  module.canvas = canvas;
  state.overlayEl = document.getElementById("overlay");
  state.detailEl = document.getElementById("detail");
  state.statusEl = state.detailEl || document.getElementById("status");
  canvas.tabIndex = 0;
  canvas.style.outline = "none";
  canvas.style.touchAction = "none";

  module.neoloveSetAudioError = (error) => {
    const message = String((error && error.message) || error || "unknown audio error");
    state.audio.lastError = message;
    console.error(message);
  };

  module.neoloveClearAudioError = () => {
    state.audio.lastError = "";
  };

  if (!state.media) {
    state.media = {
      queue: [],
      current: null,
      cancelled: new Set(),
      streams: new Map(),
      stoppedErrors: new Map(),
      nextStreamId: 1,
      permissions: { microphone: "prompt", camera: "prompt" },
      permissionQueries: { microphone: false, camera: false },
      cleanupInstalled: false
    };
  }

  module.neoloveMediaError = (error) => {
    const name = String((error && error.name) || "");
    const message = String((error && error.message) || error || "media capture failed");
    let code = "capture_failed";
    if (name === "NotAllowedError" || name === "SecurityError") {
      code = "permission_denied";
    } else if (name === "NotFoundError" || name === "DevicesNotFoundError") {
      code = "device_unavailable";
    } else if (name === "NotReadableError" || name === "TrackStartError") {
      code = "device_busy";
    } else if (name === "OverconstrainedError" || name === "ConstraintNotSatisfiedError") {
      code = "constraints_unsatisfied";
    } else if (name === "TypeError" || name === "NotSupportedError") {
      code = "unsupported";
    } else if (name === "AbortError") {
      code = "capture_failed";
    }
    return { code, message };
  };

  module.neoloveDisposeMediaEntry = (entry) => {
    if (!entry || entry.stopped) {
      return;
    }
    entry.stopped = true;
    try {
      entry.stream.getTracks().forEach((track) => track.stop());
    } catch (_error) {
    }
    if (entry.audio) {
      entry.audio.processor.onaudioprocess = null;
      try { entry.audio.source.disconnect(); } catch (_error) {}
      try { entry.audio.processor.disconnect(); } catch (_error) {}
      try { entry.audio.silence.disconnect(); } catch (_error) {}
      entry.audio.chunks.length = 0;
      entry.audio.available = 0;
    }
    if (entry.video) {
      try { entry.video.element.pause(); } catch (_error) {}
      entry.video.element.srcObject = null;
      entry.video.currentPixels = null;
    }
  };

  module.neoloveStopMediaStream = (streamId) => {
    const media = state.media;
    const entry = media && media.streams.get(streamId);
    if (!entry) {
      return false;
    }
    media.streams.delete(streamId);
    if (entry.lastError) {
      media.stoppedErrors.set(streamId, entry.lastError);
      if (media.stoppedErrors.size > 128) {
        media.stoppedErrors.delete(media.stoppedErrors.keys().next().value);
      }
    }
    module.neoloveDisposeMediaEntry(entry);
    return true;
  };

  if (!state.media.cleanupInstalled) {
    const stopAllMedia = () => {
      const ids = Array.from(state.media.streams.keys());
      ids.forEach((id) => module.neoloveStopMediaStream(id));
    };
    window.addEventListener("pagehide", stopAllMedia);
    window.addEventListener("beforeunload", stopAllMedia);
    state.media.cleanupInstalled = true;
  }

  module.neoloveEnsureAudioContext = () => {
    const AudioContextCtor = window.AudioContext || window.webkitAudioContext;
    if (!AudioContextCtor) {
      throw new Error("Web Audio API is not available in this browser");
    }
    if (!state.audio.context) {
      state.audio.context = new AudioContextCtor();
    }
    return state.audio.context;
  };

  module.neoloveStopAudioInstance = (soundId) => {
    const existing = state.audio.active.get(soundId);
    if (!existing) {
      return;
    }
    existing.stopped = true;
    state.audio.active.delete(soundId);
    try {
      if (existing.source) {
        existing.source.onended = null;
        existing.source.stop();
      }
    } catch (_error) {
    }
    try {
      if (existing.source) {
        existing.source.disconnect();
      }
    } catch (_error) {
    }
    try {
      if (existing.gain) {
        existing.gain.disconnect();
      }
    } catch (_error) {
    }
    try {
      if (existing.panner) {
        existing.panner.disconnect();
      }
    } catch (_error) {
    }
  };

  module.neoloveStopAudio3DInstance = (voiceId) => {
    const existing = state.audio.active3d.get(voiceId);
    if (!existing) {
      return;
    }
    existing.stopped = true;
    state.audio.active3d.delete(voiceId);
    try {
      if (existing.source) {
        existing.source.onended = null;
        existing.source.stop();
      }
    } catch (_error) {
    }
    try { if (existing.source) existing.source.disconnect(); } catch (_error) {}
    try { if (existing.gain) existing.gain.disconnect(); } catch (_error) {}
    try { if (existing.panner) existing.panner.disconnect(); } catch (_error) {}
  };

  if (!state.audio.resumeHooksInstalled) {
    const resumeAudio = () => {
      try {
        const context = module.neoloveEnsureAudioContext();
        if (context.state === "suspended") {
          const promise = context.resume();
          if (promise && typeof promise.catch === "function") {
            promise.catch(module.neoloveSetAudioError);
          }
        }
      } catch (error) {
        module.neoloveSetAudioError(error);
      }
    };
    window.addEventListener("pointerdown", resumeAudio, { passive: true });
    window.addEventListener("keydown", resumeAudio, { passive: true });
    window.addEventListener("touchstart", resumeAudio, { passive: true });
    state.audio.resumeHooksInstalled = true;
  }

  const normalizeKey = (event) => {
    switch (event.code) {
      case "ShiftLeft": return "leftshift";
      case "ShiftRight": return "rightshift";
      case "ControlLeft": return "leftcontrol";
      case "ControlRight": return "rightcontrol";
      case "AltLeft": return "leftalt";
      case "AltRight": return "rightalt";
      case "MetaLeft": return "leftsuper";
      case "MetaRight": return "rightsuper";
      default: break;
    }

    switch (event.key) {
      case " ": return "space";
      case "Escape": return "escape";
      case "Enter": return "enter";
      case "Tab": return "tab";
      case "Backspace": return "backspace";
      case "ArrowLeft": return "left";
      case "ArrowRight": return "right";
      case "ArrowUp": return "up";
      case "ArrowDown": return "down";
      default: break;
    }

    if (/^[a-zA-Z]$/.test(event.key)) {
      return event.key.toLowerCase();
    }
    if (/^[0-9]$/.test(event.key)) {
      return event.key;
    }
    if (/^F([1-9]|1[0-2])$/.test(event.key)) {
      return event.key.toLowerCase();
    }
    return null;
  };

  const buttonIndex = (button) => {
    switch (button) {
      case 0: return 0;
      case 1: return 1;
      case 2: return 2;
      default: return 3;
    }
  };

  const shouldAllowBrowserShortcut = (event) => {
    const key = typeof event.key === "string" ? event.key.toLowerCase() : "";
    if (key === "f12") {
      return true;
    }
    const primaryModifier = event.ctrlKey || event.metaKey;
    const secondaryModifier = event.shiftKey || event.altKey;
    if (!primaryModifier || !secondaryModifier) {
      return false;
    }
    return key === "i" || key === "j" || key === "c";
  };

  const shouldOpenBrowserContextMenu = (event) => {
    return event.button === 2 && event.shiftKey;
  };

  const syncCanvasSize = () => {
    const rect = canvas.getBoundingClientRect();
    let logicalWidth = Math.max(rect.width, canvas.clientWidth, 1);
    let logicalHeight = Math.max(rect.height, canvas.clientHeight, 1);
    if (logicalWidth <= 1 && window.innerWidth > 1) {
      logicalWidth = window.innerWidth;
    }
    if (logicalHeight <= 1 && window.innerHeight > 1) {
      logicalHeight = window.innerHeight;
    }
    const dpr = window.devicePixelRatio || 1;
    const width = Math.max(1, Math.round(logicalWidth * dpr));
    const height = Math.max(1, Math.round(logicalHeight * dpr));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
      state.imageData = null;
      if (state.ctx) {
        state.ctx.imageSmoothingEnabled = false;
      }
    }
    state.canvasWidth = canvas.width || width;
    state.canvasHeight = canvas.height || height;
  };
  module.neoloveSyncCanvasSize = syncCanvasSize;

  const queueCanvasSizeRefresh = () => {
    if (state.resizeRafQueued) {
      return;
    }
    state.resizeRafQueued = true;
    requestAnimationFrame(() => {
      state.resizeRafQueued = false;
      syncCanvasSize();
    });
  };

  const updateMouse = (event) => {
    const rect = canvas.getBoundingClientRect();
    const logicalWidth = rect.width || 1;
    const logicalHeight = rect.height || 1;
    state.mouseX = ((event.clientX - rect.left) * state.canvasWidth) / logicalWidth;
    state.mouseY = ((event.clientY - rect.top) * state.canvasHeight) / logicalHeight;
  };

  if (!state.initialized) {
    canvas.addEventListener("contextmenu", (event) => {
      if (shouldOpenBrowserContextMenu(event)) {
        return;
      }
      event.preventDefault();
    });

    canvas.addEventListener("mousedown", (event) => {
      if (shouldOpenBrowserContextMenu(event)) {
        return;
      }
      canvas.focus();
      updateMouse(event);
      const index = buttonIndex(event.button);
      if (!state.buttonsDown[index]) {
        state.buttonsPressed[index] = 1;
      }
      state.buttonsDown[index] = 1;
      event.preventDefault();
    });

    window.addEventListener("mouseup", (event) => {
      const index = buttonIndex(event.button);
      if (state.buttonsDown[index]) {
        state.buttonsReleased[index] = 1;
      }
      state.buttonsDown[index] = 0;
    });

    canvas.addEventListener("mousemove", (event) => {
      updateMouse(event);
    });

    canvas.addEventListener("wheel", (event) => {
      state.wheelX += event.deltaX / 40.0;
      state.wheelY += event.deltaY / -40.0;
      updateMouse(event);
      event.preventDefault();
    }, { passive: false });

    window.addEventListener("keydown", (event) => {
      if (shouldAllowBrowserShortcut(event)) {
        return;
      }
      const key = normalizeKey(event);
      if (!key) {
        return;
      }
      if (!state.keysDown[key]) {
        state.keysPressed[key] = 1;
      }
      state.keysDown[key] = 1;
      state.lastKey = key;
      if (
        event.key.length === 1 &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.metaKey
      ) {
        state.charPressed = event.key;
      }
      event.preventDefault();
    });

    window.addEventListener("keyup", (event) => {
      if (shouldAllowBrowserShortcut(event)) {
        return;
      }
      const key = normalizeKey(event);
      if (!key) {
        return;
      }
      if (state.keysDown[key]) {
        state.keysReleased[key] = 1;
      }
      delete state.keysDown[key];
      event.preventDefault();
    });

    window.addEventListener("resize", syncCanvasSize);
    if (!state.resizeObserver && typeof ResizeObserver === "function") {
      state.resizeObserver = new ResizeObserver(() => {
        queueCanvasSizeRefresh();
      });
      state.resizeObserver.observe(canvas);
      if (canvas.parentElement) {
        state.resizeObserver.observe(canvas.parentElement);
      }
    }
    state.initialized = true;
  }

  syncCanvasSize();
  queueCanvasSizeRefresh();

  if (state.statusEl) {
    state.statusEl.textContent = "Loading...";
    state.statusEl.dataset.state = "loading";
  }
});

EM_JS(int, neolove_js_canvas_width, (), {
  return Module.neoloveState ? Module.neoloveState.canvasWidth : 1;
});

EM_JS(int, neolove_js_canvas_height, (), {
  return Module.neoloveState ? Module.neoloveState.canvasHeight : 1;
});

EM_JS(double, neolove_js_mouse_x, (), {
  return Module.neoloveState ? Module.neoloveState.mouseX : 0.0;
});

EM_JS(double, neolove_js_mouse_y, (), {
  return Module.neoloveState ? Module.neoloveState.mouseY : 0.0;
});

EM_JS(int, neolove_js_mouse_button_state, (int index, int kind), {
  const state = Module.neoloveState;
  if (!state) {
    return 0;
  }
  if (index < 0 || index >= state.buttonsDown.length) {
    return 0;
  }
  if (kind === 0) {
    return state.buttonsDown[index] ? 1 : 0;
  }
  if (kind === 1) {
    return state.buttonsPressed[index] ? 1 : 0;
  }
  if (kind === 2) {
    return state.buttonsReleased[index] ? 1 : 0;
  }
  return 0;
});

EM_JS(double, neolove_js_wheel_x, (), {
  return Module.neoloveState ? Module.neoloveState.wheelX : 0.0;
});

EM_JS(double, neolove_js_wheel_y, (), {
  return Module.neoloveState ? Module.neoloveState.wheelY : 0.0;
});

EM_JS(int, neolove_js_key_state, (const char* name, int kind), {
  const state = Module.neoloveState;
  if (!state) {
    return 0;
  }
  const key = UTF8ToString(name);
  if (kind === 0) {
    return state.keysDown[key] ? 1 : 0;
  }
  if (kind === 1) {
    return state.keysPressed[key] ? 1 : 0;
  }
  if (kind === 2) {
    return state.keysReleased[key] ? 1 : 0;
  }
  return 0;
});

EM_JS(int, neolove_js_take_last_key, (char* buffer, int capacity), {
  const state = Module.neoloveState;
  if (!state || !state.lastKey || capacity <= 0) {
    return 0;
  }
  const value = state.lastKey;
  state.lastKey = "";
  const required = lengthBytesUTF8(value) + 1;
  if (required > capacity) {
    return -required;
  }
  stringToUTF8(value, buffer, capacity);
  return required - 1;
});

EM_JS(int, neolove_js_take_char, (char* buffer, int capacity), {
  const state = Module.neoloveState;
  if (!state || !state.charPressed || capacity <= 0) {
    return 0;
  }
  const value = state.charPressed;
  state.charPressed = "";
  const required = lengthBytesUTF8(value) + 1;
  if (required > capacity) {
    return -required;
  }
  stringToUTF8(value, buffer, capacity);
  return required - 1;
});

EM_JS(void, neolove_js_begin_frame, (), {
  const state = Module.neoloveState;
  if (!state) {
    return;
  }
  state.buttonsPressed.fill(0);
  state.buttonsReleased.fill(0);
  state.keysPressed = Object.create(null);
  state.keysReleased = Object.create(null);
  state.wheelX = 0;
  state.wheelY = 0;
  state.lastKey = "";
  state.charPressed = "";
});

EM_JS(void, neolove_js_present_rgba, (const uint8_t* pixels, int width, int height), {
  const state = Module.neoloveState;
  if (!state) {
    return;
  }
  if (Module.neoloveSyncCanvasSize) {
    Module.neoloveSyncCanvasSize();
  }
  const canvas = Module.canvas;
  if (!canvas) {
    return;
  }
  if (!state.ctx) {
    state.ctx = canvas.getContext("2d", { alpha: false, desynchronized: true }) || canvas.getContext("2d");
    if (state.ctx) {
      state.ctx.imageSmoothingEnabled = false;
    }
  }
  if (!state.ctx) {
    if (!state.debugMissingContextLogged) {
      state.debugMissingContextLogged = true;
      console.warn("[NeoLOVE debug] failed to acquire a 2D canvas context");
    }
    return;
  }
  if (!state.imageData || state.imageData.width !== width || state.imageData.height !== height) {
    state.imageData = state.ctx.createImageData(width, height);
  }
  const expectedBytes = width * height * 4;
  const view = HEAPU8.subarray(pixels, pixels + expectedBytes);
  try {
    state.imageData.data.set(view);
    state.ctx.putImageData(state.imageData, 0, 0);
  } catch (error) {
    console.warn("[NeoLOVE debug] putImageData failed", error);
  }
});

EM_JS(int, neolove_js_capture_rgba, (uint8_t* pixels, int width, int height), {
  const state = Module.neoloveState;
  const canvas = Module.canvas;
  if (!state || !canvas || !pixels || width <= 0 || height <= 0) {
    return 0;
  }
  if (!state.ctx) {
    state.ctx = canvas.getContext("2d", { alpha: false, desynchronized: true }) || canvas.getContext("2d");
  }
  if (!state.ctx) {
    return 0;
  }
  try {
    const frame = state.ctx.getImageData(0, 0, width, height);
    HEAPU8.set(frame.data, pixels);
    return 1;
  } catch (error) {
    if (!state.captureFailedLogged) {
      state.captureFailedLogged = true;
      console.warn("NeoLOVE could not capture the web canvas for a full-frame effect", error);
    }
    return 0;
  }
});


EM_JS(void, neolove_js_clear_canvas, (int r, int g, int b, int a), {
  const state = Module.neoloveState;
  const canvas = Module.canvas;
  if (!state || !canvas) {
    return;
  }
  if (!state.ctx) {
    state.ctx = canvas.getContext("2d", { alpha: false, desynchronized: true }) || canvas.getContext("2d");
    if (state.ctx) {
      state.ctx.imageSmoothingEnabled = false;
    }
  }
  const ctx = state.ctx;
  if (!ctx) {
    return;
  }
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.globalCompositeOperation = "source-over";
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = `rgba(${r}, ${g}, ${b}, ${Math.max(0, Math.min(1, a / 255))})`;
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.restore();
});

EM_JS(void, neolove_js_composite_rgba, (const uint8_t* pixels, int width, int height, int dst_x, int dst_y), {
  const state = Module.neoloveState;
  const canvas = Module.canvas;
  if (!state || !canvas || width <= 0 || height <= 0) {
    return;
  }
  if (!state.ctx) {
    state.ctx = canvas.getContext("2d", { alpha: false, desynchronized: true }) || canvas.getContext("2d");
    if (state.ctx) {
      state.ctx.imageSmoothingEnabled = false;
    }
  }
  const ctx = state.ctx;
  if (!ctx) {
    return;
  }
  const blitCanvas = state.blitCanvas || (state.blitCanvas = document.createElement("canvas"));
  if (blitCanvas.width !== width || blitCanvas.height !== height) {
    blitCanvas.width = width;
    blitCanvas.height = height;
    state.blitImageData = null;
  }
  const blitCtx = state.blitCtx || (state.blitCtx = blitCanvas.getContext("2d"));
  if (!blitCtx) {
    return;
  }
  if (!state.blitImageData || state.blitImageData.width !== width || state.blitImageData.height !== height) {
    state.blitImageData = blitCtx.createImageData(width, height);
  }
  const expectedBytes = width * height * 4;
  state.blitImageData.data.set(HEAPU8.subarray(pixels, pixels + expectedBytes));
  blitCtx.putImageData(state.blitImageData, 0, 0);
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.globalCompositeOperation = "source-over";
  ctx.drawImage(blitCanvas, dst_x, dst_y);
  ctx.restore();
});

EM_JS(void, neolove_js_draw_image, (
  uintptr_t image_id,
  double revision,
  const uint8_t* pixels,
  int image_width,
  int image_height,
  float source_x,
  float source_y,
  float source_w,
  float source_h,
  float dest_x,
  float dest_y,
  float dest_w,
  float dest_h,
  float rotation,
  float pivot_x,
  float pivot_y,
  float alpha,
  int linear_filter
), {
  const state = Module.neoloveState;
  const canvas = Module.canvas;
  if (!state || !canvas || image_width <= 0 || image_height <= 0 ||
      source_w <= 0 || source_h <= 0 || dest_w <= 0 || dest_h <= 0) {
    return;
  }
  if (!state.ctx) {
    state.ctx = canvas.getContext("2d", { alpha: false, desynchronized: true }) || canvas.getContext("2d");
  }
  const ctx = state.ctx;
  if (!ctx) {
    return;
  }

  state.images ||= new Map();
  state.imageUseCounter = (state.imageUseCounter || 0) + 1;
  let cached = state.images.get(image_id);
  if (!cached || cached.revision !== revision ||
      cached.canvas.width !== image_width || cached.canvas.height !== image_height) {
    const imageCanvas = cached ? cached.canvas : document.createElement("canvas");
    imageCanvas.width = image_width;
    imageCanvas.height = image_height;
    const imageCtx = imageCanvas.getContext("2d");
    if (!imageCtx) {
      return;
    }
    const byteLength = image_width * image_height * 4;
    const copiedPixels = new Uint8ClampedArray(byteLength);
    copiedPixels.set(HEAPU8.subarray(pixels, pixels + byteLength));
    imageCtx.putImageData(new ImageData(copiedPixels, image_width, image_height), 0, 0);
    cached = { canvas: imageCanvas, revision: revision, lastUsed: state.imageUseCounter };
    state.images.set(image_id, cached);
    if (state.images.size > 256) {
      let oldestId = null;
      let oldestUse = Number.POSITIVE_INFINITY;
      for (const [cachedId, entry] of state.images) {
        if (entry.lastUsed < oldestUse) {
          oldestId = cachedId;
          oldestUse = entry.lastUsed;
        }
      }
      if (oldestId !== null && oldestId !== image_id) {
        state.images.delete(oldestId);
      }
    }
  }
  cached.lastUsed = state.imageUseCounter;

  ctx.save();
  ctx.imageSmoothingEnabled = linear_filter !== 0;
  ctx.globalAlpha = Math.max(0, Math.min(1, alpha));
  ctx.translate(pivot_x, pivot_y);
  ctx.rotate(rotation);
  ctx.translate(-pivot_x, -pivot_y);
  ctx.drawImage(
    cached.canvas,
    source_x,
    source_y,
    source_w,
    source_h,
    dest_x,
    dest_y,
    dest_w,
    dest_h
  );
  ctx.restore();
});

EM_JS(void, neolove_js_draw_shader, (
  const char* fragment_source_ptr,
  const char* uniforms_json_ptr,
  const float* vertices_ptr,
  int vertex_count,
  uintptr_t texture_id,
  double texture_revision,
  const uint8_t* texture_pixels,
  int texture_width,
  int texture_height,
  int linear_filter,
  int antialiasing_mode,
  int depth_test
), {
  const state = Module.neoloveState;
  const canvas = Module.canvas;
  if (!state || !canvas || vertex_count <= 0) return;

  const fragmentSource = UTF8ToString(fragment_source_ptr);
  let uniforms = {};
  try {
    uniforms = JSON.parse(UTF8ToString(uniforms_json_ptr) || "{}");
  } catch (error) {
    console.error("NeoLOVE shader uniform JSON error", error);
  }

  const aaMode = Math.max(0, Math.min(2, antialiasing_mode | 0));
  const requestedRenderScale = aaMode === 2 ? 2 : 1;
  const contextKey = aaMode === 0 ? 0 : 1;
  state.shaderRenderers = state.shaderRenderers || new Map();
  let shaderRenderer = state.shaderRenderers.get(contextKey);
  if (!shaderRenderer) {
    const rendererCanvas = document.createElement("canvas");
    rendererCanvas.width = 1;
    rendererCanvas.height = 1;
    const contextOptions = {
      alpha: true,
      premultipliedAlpha: false,
      antialias: contextKey !== 0,
      depth: true
    };
    const rendererGl = rendererCanvas.getContext("webgl", contextOptions)
      || rendererCanvas.getContext("experimental-webgl", contextOptions);
    shaderRenderer = {
      canvas: rendererCanvas,
      gl: rendererGl,
      programs: new Map(),
      textures: new Map(),
      vertexBuffer: null,
      whiteTexture: null,
      whiteTextureInitialized: false
    };
    state.shaderRenderers.set(contextKey, shaderRenderer);
  }
  const glCanvas = shaderRenderer.canvas;
  const gl = shaderRenderer.gl;
  if (!gl) {
    if (!state.shaderMissingWebglLogged) {
      state.shaderMissingWebglLogged = true;
      console.error("NeoLOVE web shaders require WebGL support in this browser");
    }
    return;
  }
  const maximumRenderbufferSize = gl.getParameter(gl.MAX_RENDERBUFFER_SIZE) || 0;
  const renderScale = requestedRenderScale === 2
    && canvas.width * 2 <= maximumRenderbufferSize
    && canvas.height * 2 <= maximumRenderbufferSize ? 2 : 1;
  const targetWidth = Math.max(1, canvas.width * renderScale);
  const targetHeight = Math.max(1, canvas.height * renderScale);
  if (glCanvas.width !== targetWidth || glCanvas.height !== targetHeight) {
    glCanvas.width = targetWidth;
    glCanvas.height = targetHeight;
  }

  const vertexSource = `
attribute vec3 a_pos;
attribute vec2 a_uv;
attribute vec4 a_color;
varying mediump vec2 uv;
varying mediump vec4 color;
void main() {
  gl_Position = vec4(a_pos, 1.0);
  uv = a_uv;
  color = a_color;
}`;
  const compileShader = (type, source) => {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      const log = gl.getShaderInfoLog(shader) || "unknown shader compile error";
      gl.deleteShader(shader);
      throw new Error(log);
    }
    return shader;
  };

  let programInfo = shaderRenderer.programs.get(fragmentSource);
  if (!programInfo) {
    try {
      const vs = compileShader(gl.VERTEX_SHADER, vertexSource);
      const fs = compileShader(gl.FRAGMENT_SHADER, fragmentSource);
      const program = gl.createProgram();
      gl.attachShader(program, vs);
      gl.attachShader(program, fs);
      gl.linkProgram(program);
      gl.deleteShader(vs);
      gl.deleteShader(fs);
      if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
        const log = gl.getProgramInfoLog(program) || "unknown shader link error";
        gl.deleteProgram(program);
        throw new Error(log);
      }
      programInfo = {
        program,
        aPos: gl.getAttribLocation(program, "a_pos"),
        aUv: gl.getAttribLocation(program, "a_uv"),
        aColor: gl.getAttribLocation(program, "a_color"),
        uTexture: gl.getUniformLocation(program, "Texture"),
        uniformLocations: new Map()
      };
      shaderRenderer.programs.set(fragmentSource, programInfo);
    } catch (error) {
      console.error("NeoLOVE web shader compile failed", error);
      return;
    }
  }

  const packed = new Float32Array(HEAPF32.subarray(vertices_ptr >> 2, (vertices_ptr >> 2) + vertex_count * 9));
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (let index = 0; index < vertex_count; index += 1) {
    const offset = index * 9;
    const x = packed[offset];
    const y = packed[offset + 1];
    minX = Math.min(minX, x); minY = Math.min(minY, y);
    maxX = Math.max(maxX, x); maxY = Math.max(maxY, y);
    packed[offset] = (x / Math.max(1, canvas.width)) * 2 - 1;
    packed[offset + 1] = 1 - (y / Math.max(1, canvas.height)) * 2;
    packed[offset + 2] = Math.max(-1, Math.min(1, packed[offset + 2] * 2 - 1));
  }

  const buffer = shaderRenderer.vertexBuffer || (shaderRenderer.vertexBuffer = gl.createBuffer());
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, packed, gl.STREAM_DRAW);
  gl.viewport(0, 0, glCanvas.width, glCanvas.height);
  if (depth_test) {
    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LEQUAL);
    gl.depthMask(true);
  } else {
    gl.disable(gl.DEPTH_TEST);
  }
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
  gl.clearColor(0, 0, 0, 0);
  gl.clearDepth(1.0);
  gl.clear(gl.COLOR_BUFFER_BIT | (depth_test ? gl.DEPTH_BUFFER_BIT : 0));
  gl.useProgram(programInfo.program);

  const stride = 9 * 4;
  if (programInfo.aPos >= 0) {
    gl.enableVertexAttribArray(programInfo.aPos);
    gl.vertexAttribPointer(programInfo.aPos, 3, gl.FLOAT, false, stride, 0);
  }
  if (programInfo.aUv >= 0) {
    gl.enableVertexAttribArray(programInfo.aUv);
    gl.vertexAttribPointer(programInfo.aUv, 2, gl.FLOAT, false, stride, 3 * 4);
  }
  if (programInfo.aColor >= 0) {
    gl.enableVertexAttribArray(programInfo.aColor);
    gl.vertexAttribPointer(programInfo.aColor, 4, gl.FLOAT, false, stride, 5 * 4);
  }

  let texture;
  if (texture_pixels && texture_width > 0 && texture_height > 0) {
    let cached = shaderRenderer.textures.get(texture_id);
    if (!cached || cached.revision !== texture_revision || cached.width !== texture_width || cached.height !== texture_height) {
      if (cached) gl.deleteTexture(cached.texture);
      texture = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, texture_width, texture_height, 0, gl.RGBA, gl.UNSIGNED_BYTE,
        HEAPU8.subarray(texture_pixels, texture_pixels + texture_width * texture_height * 4));
      cached = { texture, revision: texture_revision, width: texture_width, height: texture_height };
      shaderRenderer.textures.set(texture_id, cached);
    } else {
      texture = cached.texture;
      gl.bindTexture(gl.TEXTURE_2D, texture);
    }
    const filter = linear_filter ? gl.LINEAR : gl.NEAREST;
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
  } else {
    texture = shaderRenderer.whiteTexture || (shaderRenderer.whiteTexture = gl.createTexture());
    gl.bindTexture(gl.TEXTURE_2D, texture);
    if (!shaderRenderer.whiteTextureInitialized) {
      shaderRenderer.whiteTextureInitialized = true;
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([255, 255, 255, 255]));
    }
  }
  gl.activeTexture(gl.TEXTURE0);
  gl.bindTexture(gl.TEXTURE_2D, texture);
  if (programInfo.uTexture !== null) gl.uniform1i(programInfo.uTexture, 0);

  const floats = uniforms.floats || {};
  for (const name of Object.keys(floats)) {
    let location = programInfo.uniformLocations.get(name);
    if (location === undefined) {
      location = gl.getUniformLocation(programInfo.program, name);
      programInfo.uniformLocations.set(name, location);
    }
    if (location === null) continue;
    const values = floats[name] || [];
    if (values.length === 1) gl.uniform1f(location, values[0]);
    else if (values.length === 2) gl.uniform2f(location, values[0], values[1]);
    else if (values.length === 3) gl.uniform3f(location, values[0], values[1], values[2]);
    else if (values.length >= 4) gl.uniform4f(location, values[0], values[1], values[2], values[3]);
  }

  gl.drawArrays(gl.TRIANGLES, 0, vertex_count);
  if (!state.ctx) state.ctx = canvas.getContext("2d", { alpha: false, desynchronized: true }) || canvas.getContext("2d");
  if (state.ctx) {
    const sx = Math.max(0, Math.floor(minX));
    const sy = Math.max(0, Math.floor(minY));
    const ex = Math.min(canvas.width, Math.ceil(maxX));
    const ey = Math.min(canvas.height, Math.ceil(maxY));
    if (ex > sx && ey > sy) {
      state.ctx.save();
      state.ctx.setTransform(1, 0, 0, 1, 0, 0);
      state.ctx.globalCompositeOperation = "source-over";
      state.ctx.drawImage(
        glCanvas,
        sx * renderScale,
        sy * renderScale,
        (ex - sx) * renderScale,
        (ey - sy) * renderScale,
        sx,
        sy,
        ex - sx,
        ey - sy
      );
      state.ctx.restore();
    }
  }
});

EM_JS(void, neolove_js_draw_text, (
  const char* message,
  float x,
  float y,
  float w,
  float h,
  float rotation,
  float pivot_x,
  float pivot_y,
  int r,
  int g,
  int b,
  int a,
  float scale,
  float min_scale,
  int align_x,
  int align_y,
  int text_scale,
  int wrap,
  float padding_x,
  float padding_y,
  float line_spacing,
  float letter_spacing,
  int font_kind,
  const char* font_path_ptr
), {
  const state = Module.neoloveState;
  if (!state || !Module.canvas) {
    return;
  }
  if (!state.ctx) {
    state.ctx = Module.canvas.getContext("2d", { alpha: false, desynchronized: true }) || Module.canvas.getContext("2d");
    if (state.ctx) {
      state.ctx.imageSmoothingEnabled = false;
    }
  }
  const ctx = state.ctx;
  if (!ctx) {
    return;
  }

  const text = UTF8ToString(message);
  if (!text || !text.length || a <= 0) {
    return;
  }

  const paddingX = Math.max(0, padding_x);
  const paddingY = Math.max(0, padding_y);
  const preferredScale = Math.max(1, scale);
  const minimumScale = Math.max(1, Math.min(min_scale, preferredScale));
  const effectiveLineSpacing = Math.max(0.1, line_spacing);
  const widthLimit = w > 0 ? Math.max(0, w - paddingX * 2) : null;
  const heightLimit = h > 0 ? Math.max(0, h - paddingY * 2) : null;
  const activeWrap = widthLimit !== null && wrap !== 0 ? wrap : 0;
  const fontPath = font_path_ptr ? UTF8ToString(font_path_ptr) : "";
  const hashString = (value) => {
    let hash = 0;
    for (let i = 0; i < value.length; i += 1) {
      hash = ((hash << 5) - hash + value.charCodeAt(i)) | 0;
    }
    return hash;
  };
  const ensureCustomFont = (path) => {
    if (!path || font_kind === 0 || typeof FontFace === "undefined" || typeof FS === "undefined") {
      return null;
    }
    state.fonts = state.fonts || new Map();
    let entry = state.fonts.get(path);
    if (entry) {
      return entry;
    }
    const family = `NeoLOVE_${state.fonts.size}_${Math.abs(hashString(path))}`;
    entry = { family, loaded: false, failed: false };
    state.fonts.set(path, entry);
    try {
      const bytes = FS.readFile(path);
      const face = new FontFace(family, bytes);
      entry.face = face;
      face.load().then((loadedFace) => {
        document.fonts.add(loadedFace);
        entry.loaded = true;
      }).catch((error) => {
        entry.failed = true;
        if (!entry.logged) {
          entry.logged = true;
          console.warn(`NeoLOVE failed to load web font '${path}'`, error);
        }
      });
    } catch (error) {
      entry.failed = true;
      if (!entry.logged) {
        entry.logged = true;
        console.warn(`NeoLOVE failed to read web font '${path}'`, error);
      }
    }
    return entry;
  };
  const customFont = ensureCustomFont(fontPath);
  const family = customFont && customFont.loaded ? customFont.family :
    (font_kind === 0 ? "monospace" : "sans-serif");
  const safeLetterSpacing = Number.isFinite(letter_spacing) ? letter_spacing : 0;

  const setFont = (px) => {
    ctx.font = `${Math.max(1, px)}px ${family}`;
  };

  const measureLine = (line) => {
    const textValue = String(line || "");
    if (!textValue.length) {
      return 0;
    }
    const glyphs = Array.from(textValue);
    let width = 0;
    for (let i = 0; i < glyphs.length; i += 1) {
      if (i > 0) {
        width += safeLetterSpacing;
      }
      width += ctx.measureText(glyphs[i]).width;
    }
    return width;
  };

  const wrapParagraphChar = (paragraph, limit) => {
    if (!(limit > 0)) {
      return [paragraph];
    }
    const chars = Array.from(paragraph);
    const lines = [];
    let current = "";
    for (const ch of chars) {
      const candidate = current + ch;
      if (current && measureLine(candidate) > limit + 0.5) {
        lines.push(current);
        current = ch;
      } else {
        current = candidate;
      }
    }
    if (current.length || lines.length === 0) {
      lines.push(current);
    }
    return lines;
  };

  const wrapParagraphWord = (paragraph, limit) => {
    if (!(limit > 0)) {
      return [paragraph];
    }
    const trimmed = paragraph.trim();
    if (!trimmed.length) {
      return [""];
    }
    const words = trimmed.split(" ").filter((part) => part.length > 0);
    const lines = [];
    let current = "";
    for (const word of words) {
      const candidate = current ? `${current} ${word}` : word;
      if (!current || measureLine(candidate) <= limit + 0.5) {
        current = candidate;
        continue;
      }
      if (measureLine(word) > limit + 0.5) {
        if (current) {
          lines.push(current);
          current = "";
        }
        const fragments = wrapParagraphChar(word, limit);
        for (let index = 0; index < fragments.length - 1; index += 1) {
          lines.push(fragments[index]);
        }
        current = fragments[fragments.length - 1] || "";
      } else {
        lines.push(current);
        current = word;
      }
    }
    if (current.length || lines.length === 0) {
      lines.push(current);
    }
    return lines;
  };

  const layoutForScale = (px) => {
    setFont(px);
    const paragraphs = String(text).split("\n");
    const lines = [];
    for (const paragraph of paragraphs) {
      let wrapped;
      if (activeWrap === 1) {
        wrapped = wrapParagraphWord(paragraph, widthLimit);
      } else if (activeWrap === 2) {
        wrapped = wrapParagraphChar(paragraph, widthLimit);
      } else {
        wrapped = [paragraph];
      }
      for (const line of wrapped) {
        lines.push({ text: line, width: measureLine(line) });
      }
    }
    const lineHeight = Math.max(1, px * effectiveLineSpacing);
    const blockWidth = lines.reduce((maxWidth, line) => Math.max(maxWidth, line.width), 0);
    const blockHeight = lines.length > 0 ? px + lineHeight * (lines.length - 1) : 0;
    return { lines, lineHeight, blockWidth, blockHeight, px };
  };

  const fits = (layout) => {
    switch (text_scale | 0) {
      case 1:
        return (widthLimit === null || layout.blockWidth <= widthLimit + 0.5) &&
               (heightLimit === null || layout.blockHeight <= heightLimit + 0.5);
      case 2:
        return widthLimit === null || layout.blockWidth <= widthLimit + 0.5;
      case 3:
        return heightLimit === null || layout.blockHeight <= heightLimit + 0.5;
      default:
        return true;
    }
  };

  let layout = layoutForScale(preferredScale);
  let usedScale = preferredScale;
  if ((text_scale | 0) !== 0 && (widthLimit !== null || heightLimit !== null) && !fits(layout)) {
    let low = minimumScale;
    let high = preferredScale;
    let bestScale = minimumScale;
    let bestLayout = layoutForScale(minimumScale);
    if (fits(bestLayout)) {
      for (let i = 0; i < 10; i += 1) {
        const mid = (low + high) * 0.5;
        const candidate = layoutForScale(mid);
        if (fits(candidate)) {
          bestScale = mid;
          bestLayout = candidate;
          low = mid;
        } else {
          high = mid;
        }
      }
    }
    usedScale = bestScale;
    layout = bestLayout;
  }

  ctx.save();
  setFont(usedScale);
  ctx.fillStyle = `rgba(${r}, ${g}, ${b}, ${Math.max(0, Math.min(1, a / 255))})`;
  ctx.textBaseline = "top";
  ctx.textAlign = "left";
  ctx.imageSmoothingEnabled = false;

  if (rotation !== 0) {
    ctx.translate(pivot_x, pivot_y);
    ctx.rotate(rotation);
    ctx.translate(-pivot_x, -pivot_y);
  }

  if (w > 0 && h > 0) {
    ctx.beginPath();
    ctx.rect(x, y, w, h);
    ctx.clip();
  }

  const contentWidth = widthLimit !== null ? widthLimit : layout.blockWidth;
  const contentHeight = heightLimit !== null ? heightLimit : layout.blockHeight;
  const originX = x + paddingX;
  const originY = y + paddingY;
  const startY = originY + Math.max(0, (
    align_y === 1 ? (contentHeight - layout.blockHeight) * 0.5 :
    align_y === 2 ? (contentHeight - layout.blockHeight) :
    0
  ));

  for (let index = 0; index < layout.lines.length; index += 1) {
    const line = layout.lines[index];
    const lineX = originX + Math.max(0, (
      align_x === 1 ? (contentWidth - line.width) * 0.5 :
      align_x === 2 ? (contentWidth - line.width) :
      0
    ));
    const lineY = startY + layout.lineHeight * index;
    ctx.fillText(line.text, lineX, lineY);
  }

  ctx.restore();
});

EM_JS(void, neolove_js_report_status, (const char* message, int is_error), {
  const text = UTF8ToString(message);
  const state = Module.neoloveState;
  if (state) {
    if (state.statusEl) {
      state.statusEl.textContent = text;
    }
    if (state.overlayEl) {
      state.overlayEl.dataset.state = is_error ? "error" : "info";
    }
  }
  if (is_error) {
    console.error(text);
  } else {
    console.log(text);
  }
});


EM_JS(int, neolove_js_http_start, (
  int request_id,
  const char* url_ptr,
  const char* method_ptr,
  const char* headers_json_ptr,
  const uint8_t* body_ptr,
  int body_len
), {
  const state = Module.neoloveState;
  if (!state) {
    return 0;
  }
  const http = state.http || (state.http = { queue: [], current: null });
  try {
    const url = UTF8ToString(url_ptr);
    const method = UTF8ToString(method_ptr || 0) || "GET";
    const headersJson = UTF8ToString(headers_json_ptr || 0) || "{}";
    const headers = JSON.parse(headersJson);
    const options = { method, headers };
    if (body_len > 0 && method !== "GET" && method !== "HEAD") {
      const body = new Uint8Array(body_len);
      body.set(HEAPU8.subarray(body_ptr, body_ptr + body_len));
      options.body = body;
    }
    fetch(url, options)
      .then((response) => response.text().then((body) => {
        const responseHeaders = {};
        response.headers.forEach((value, name) => { responseHeaders[name] = value; });
        http.queue.push({
          requestId: request_id,
          url: response.url || url,
          status: response.status,
          ok: 1,
          body,
          error: "",
          headers: JSON.stringify(responseHeaders)
        });
      }))
      .catch((error) => {
        http.queue.push({
          requestId: request_id,
          url,
          status: -1,
          ok: 0,
          body: "",
          error: String((error && error.message) || error || "fetch failed"),
          headers: "{}"
        });
      });
    return 1;
  } catch (error) {
    http.queue.push({
      requestId: request_id,
      url: "",
      status: -1,
      ok: 0,
      body: "",
      error: String((error && error.message) || error || "failed to start fetch"),
      headers: "{}"
    });
    return 1;
  }
});

EM_JS(int, neolove_js_http_poll, (int* request_id, int* status, int* ok), {
  const state = Module.neoloveState;
  const http = state && state.http;
  if (!http || !http.queue.length) {
    return 0;
  }
  const event = http.queue.shift();
  http.current = event;
  HEAP32[request_id >> 2] = event.requestId | 0;
  HEAP32[status >> 2] = event.status | 0;
  HEAP32[ok >> 2] = event.ok ? 1 : 0;
  return 1;
});

EM_JS(int, neolove_js_http_copy_field, (int field, char* buffer, int capacity), {
  const state = Module.neoloveState;
  const event = state && state.http && state.http.current;
  let value = "";
  if (event) {
    switch (field) {
      case 0: value = event.url || ""; break;
      case 1: value = event.body || ""; break;
      case 2: value = event.error || ""; break;
      case 3: value = event.headers || "{}"; break;
      default: value = ""; break;
    }
  }
  const required = lengthBytesUTF8(value) + 1;
  if (capacity <= 0 || required > capacity) {
    return -required;
  }
  stringToUTF8(value, buffer, capacity);
  return required - 1;
});

EM_JS(void, neolove_js_debug_log, (const char* message), {
  console.warn("[NeoLOVE debug]", UTF8ToString(message));
});

EM_JS(void, neolove_js_mark_ready, (), {
  const state = Module.neoloveState;
  if (state) {
    if (state.statusEl) {
      state.statusEl.textContent = "";
    }
    if (state.overlayEl) {
      state.overlayEl.dataset.state = "ready";
    }
  }
});

EM_JS(int, neolove_js_audio_play, (
  int sound_id,
  const uint8_t* bytes,
  int bytes_len,
  int looped,
  float volume,
  int spatial,
  float x,
  float y
), {
  const state = Module.neoloveState;
  if (!state) {
    return 0;
  }
  try {
    Module.neoloveClearAudioError();
    const context = Module.neoloveEnsureAudioContext();
    Module.neoloveStopAudioInstance(sound_id);

    if (bytes_len <= 0) {
      throw new Error("sound has no encoded bytes");
    }

    const encodedBytes = new Uint8Array(bytes_len);
    encodedBytes.set(HEAPU8.subarray(bytes, bytes + bytes_len));

    const entry = {
      source: null,
      gain: null,
      panner: null,
      x: x,
      y: y,
      volume: Math.min(1, Math.max(0, volume)),
      stopped: false,
      started: false
    };
    const cleanupEntry = () => {
      const current = state.audio.active.get(sound_id);
      if (current !== entry) {
        return;
      }
      state.audio.active.delete(sound_id);
      try {
        if (entry.source) {
          entry.source.disconnect();
        }
      } catch (_error) {
      }
      try {
        if (entry.gain) {
          entry.gain.disconnect();
        }
      } catch (_error) {
      }
      try {
        if (entry.panner) {
          entry.panner.disconnect();
        }
      } catch (_error) {
      }
    };

    state.audio.active.set(sound_id, entry);
    const resumePromise = context.state === "running"
      ? Promise.resolve()
      : context.resume();
    const decodePromise = context.decodeAudioData(encodedBytes.buffer.slice(0));

    Promise.all([resumePromise, decodePromise])
      .then((results) => {
        const audioBuffer = results[1];
        if (entry.stopped) {
          cleanupEntry();
          return;
        }
        if (context.state !== "running") {
          throw new Error(`AudioContext state is '${context.state}' after resume`);
        }

        const gain = context.createGain();
        gain.gain.value = entry.volume;
        let output = gain;
        if (spatial) {
          const panner = context.createPanner();
          panner.panningModel = "HRTF";
          panner.distanceModel = "inverse";
          panner.refDistance = 1;
          panner.maxDistance = 10000;
          panner.rolloffFactor = 1;
          panner.positionX.value = entry.x;
          panner.positionY.value = entry.y;
          panner.positionZ.value = 0;
          gain.connect(panner);
          output = panner;
          entry.panner = panner;
        }
        output.connect(context.destination);

        const source = context.createBufferSource();
        source.buffer = audioBuffer;
        source.loop = !!looped;
        source.connect(gain);
        source.onended = () => {
          cleanupEntry();
        };

        entry.gain = gain;
        entry.source = source;
        source.start(0);
        entry.started = true;
      })
      .catch((error) => {
        cleanupEntry();
        Module.neoloveSetAudioError(error);
      });

    return 1;
  } catch (error) {
    Module.neoloveSetAudioError(error);
    return 0;
  }
});

EM_JS(int, neolove_js_audio_set_position, (int sound_id, float x, float y), {
  const state = Module.neoloveState;
  if (!state) return 0;
  try {
    const existing = state.audio.active.get(sound_id);
    if (!existing) return 0;
    existing.x = x;
    existing.y = y;
    if (existing.panner) {
      existing.panner.positionX.value = x;
      existing.panner.positionY.value = y;
    }
    return 1;
  } catch (error) {
    Module.neoloveSetAudioError(error);
    return 0;
  }
});

EM_JS(int, neolove_js_audio_set_listener_position, (float x, float y), {
  try {
    const context = Module.neoloveEnsureAudioContext();
    const listener = context.listener;
    if (listener.positionX) {
      listener.positionX.value = x;
      listener.positionY.value = y;
      listener.positionZ.value = 0;
    } else if (listener.setPosition) {
      listener.setPosition(x, y, 0);
    }
    return 1;
  } catch (error) {
    Module.neoloveSetAudioError(error);
    return 0;
  }
});

EM_JS(int, neolove_js_audio_play_spatial_3d, (
  int voice_id,
  const uint8_t* bytes,
  int bytes_len,
  int looped,
  float volume,
  float x,
  float y,
  float z,
  float min_distance,
  float max_distance,
  float rolloff,
  int distance_model
), {
  const state = Module.neoloveState;
  if (!state) return 0;
  try {
    Module.neoloveClearAudioError();
    const context = Module.neoloveEnsureAudioContext();
    Module.neoloveStopAudio3DInstance(voice_id);
    if (bytes_len <= 0) throw new Error("sound has no encoded bytes");

    const encodedBytes = new Uint8Array(bytes_len);
    encodedBytes.set(HEAPU8.subarray(bytes, bytes + bytes_len));
    const entry = {
      source: null,
      gain: null,
      panner: null,
      x, y, z,
      volume: Math.min(1, Math.max(0, volume)),
      minDistance: Math.max(0.001, min_distance),
      maxDistance: Math.max(Math.max(0.001, min_distance), max_distance),
      rolloff: Math.max(0, rolloff),
      distanceModel: distance_model,
      stopped: false
    };
    const cleanup = () => {
      if (state.audio.active3d.get(voice_id) !== entry) return;
      state.audio.active3d.delete(voice_id);
      try { if (entry.source) entry.source.disconnect(); } catch (_error) {}
      try { if (entry.gain) entry.gain.disconnect(); } catch (_error) {}
      try { if (entry.panner) entry.panner.disconnect(); } catch (_error) {}
    };
    state.audio.active3d.set(voice_id, entry);
    const resumed = context.state === "running" ? Promise.resolve() : context.resume();
    const decoded = context.decodeAudioData(encodedBytes.buffer.slice(0));
    Promise.all([resumed, decoded]).then((results) => {
      if (entry.stopped) {
        cleanup();
        return;
      }
      const gain = context.createGain();
      gain.gain.value = entry.volume;
      const panner = context.createPanner();
      panner.panningModel = "HRTF";
      panner.distanceModel = entry.distanceModel === 0
        ? "linear"
        : (entry.distanceModel === 2 ? "exponential" : "inverse");
      panner.refDistance = entry.minDistance;
      panner.maxDistance = entry.maxDistance;
      panner.rolloffFactor = entry.rolloff;
      panner.positionX.value = entry.x;
      panner.positionY.value = entry.y;
      panner.positionZ.value = entry.z;
      gain.connect(panner);
      panner.connect(context.destination);

      const source = context.createBufferSource();
      source.buffer = results[1];
      source.loop = !!looped;
      source.connect(gain);
      source.onended = cleanup;
      entry.gain = gain;
      entry.panner = panner;
      entry.source = source;
      source.start(0);
    }).catch((error) => {
      cleanup();
      Module.neoloveSetAudioError(error);
    });
    return 1;
  } catch (error) {
    Module.neoloveSetAudioError(error);
    return 0;
  }
});

EM_JS(int, neolove_js_audio_update_spatial_3d, (
  int voice_id,
  float x,
  float y,
  float z,
  float volume,
  float min_distance,
  float max_distance,
  float rolloff,
  int distance_model
), {
  const state = Module.neoloveState;
  if (!state) return 0;
  try {
    const entry = state.audio.active3d.get(voice_id);
    if (!entry) return 0;
    entry.x = x;
    entry.y = y;
    entry.z = z;
    entry.volume = Math.min(1, Math.max(0, volume));
    entry.minDistance = Math.max(0.001, min_distance);
    entry.maxDistance = Math.max(entry.minDistance, max_distance);
    entry.rolloff = Math.max(0, rolloff);
    entry.distanceModel = distance_model;
    if (entry.gain) entry.gain.gain.value = entry.volume;
    if (entry.panner) {
      entry.panner.positionX.value = x;
      entry.panner.positionY.value = y;
      entry.panner.positionZ.value = z;
      entry.panner.distanceModel = distance_model === 0
        ? "linear"
        : (distance_model === 2 ? "exponential" : "inverse");
      entry.panner.refDistance = entry.minDistance;
      entry.panner.maxDistance = entry.maxDistance;
      entry.panner.rolloffFactor = entry.rolloff;
    }
    return 1;
  } catch (error) {
    Module.neoloveSetAudioError(error);
    return 0;
  }
});

EM_JS(int, neolove_js_audio_stop_spatial_3d, (int voice_id), {
  try {
    Module.neoloveClearAudioError();
    Module.neoloveStopAudio3DInstance(voice_id);
    return 1;
  } catch (error) {
    Module.neoloveSetAudioError(error);
    return 0;
  }
});

EM_JS(int, neolove_js_audio_set_listener_3d, (
  float x,
  float y,
  float z,
  float forward_x,
  float forward_y,
  float forward_z,
  float up_x,
  float up_y,
  float up_z
), {
  try {
    const listener = Module.neoloveEnsureAudioContext().listener;
    if (listener.positionX) {
      listener.positionX.value = x;
      listener.positionY.value = y;
      listener.positionZ.value = z;
      listener.forwardX.value = forward_x;
      listener.forwardY.value = forward_y;
      listener.forwardZ.value = forward_z;
      listener.upX.value = up_x;
      listener.upY.value = up_y;
      listener.upZ.value = up_z;
    } else {
      if (listener.setPosition) listener.setPosition(x, y, z);
      if (listener.setOrientation) {
        listener.setOrientation(forward_x, forward_y, forward_z, up_x, up_y, up_z);
      }
    }
    return 1;
  } catch (error) {
    Module.neoloveSetAudioError(error);
    return 0;
  }
});

EM_JS(int, neolove_js_audio_stop, (int sound_id), {
  const state = Module.neoloveState;
  if (!state) {
    return 0;
  }
  try {
    Module.neoloveClearAudioError();
    Module.neoloveStopAudioInstance(sound_id);
    return 1;
  } catch (error) {
    Module.neoloveSetAudioError(error);
    return 0;
  }
});

EM_JS(int, neolove_js_audio_set_volume, (int sound_id, float volume), {
  const state = Module.neoloveState;
  if (!state) {
    return 0;
  }
  try {
    Module.neoloveClearAudioError();
    const existing = state.audio.active.get(sound_id);
    if (existing) {
      existing.volume = Math.min(1, Math.max(0, volume));
      if (existing.gain) {
        existing.gain.gain.value = existing.volume;
      }
    }
    return 1;
  } catch (error) {
    Module.neoloveSetAudioError(error);
    return 0;
  }
});

EM_JS(int, neolove_js_take_audio_error, (char* buffer, int capacity), {
  const state = Module.neoloveState;
  const value = state && state.audio ? state.audio.lastError : "";
  if (!value || capacity <= 0) {
    return 0;
  }
  state.audio.lastError = "";
  const required = lengthBytesUTF8(value) + 1;
  if (required > capacity) {
    return -required;
  }
  stringToUTF8(value, buffer, capacity);
  return required - 1;
});

EM_JS(int, neolove_js_media_enumerate, (int request_id, int kind), {
  const state = Module.neoloveState;
  const media = state && state.media;
  const devicesApi = navigator.mediaDevices;
  if (!media || !devicesApi || typeof devicesApi.enumerateDevices !== "function") {
    return 0;
  }
  devicesApi.enumerateDevices()
    .then((devices) => {
      if (media.cancelled.delete(request_id)) {
        return;
      }
      let audioPosition = 0;
      let videoPosition = 0;
      const result = [];
      for (const device of devices) {
        if (device.kind === "audioinput" && (kind === 0 || kind === 1)) {
          const rawId = device.deviceId || (audioPosition === 0 ? "default" : `anonymous-${audioPosition}`);
          result.push({
            id: `audio:${rawId}`,
            kind: "microphone",
            label: device.label || `Microphone ${audioPosition + 1}`,
            isDefault: rawId === "default" || audioPosition === 0
          });
          audioPosition += 1;
        } else if (device.kind === "videoinput" && (kind === 0 || kind === 2)) {
          const rawId = device.deviceId || (videoPosition === 0 ? "default" : `anonymous-${videoPosition}`);
          result.push({
            id: `camera:${rawId}`,
            kind: "camera",
            label: device.label || `Camera ${videoPosition + 1}`,
            isDefault: rawId === "default" || videoPosition === 0
          });
          videoPosition += 1;
        }
      }
      media.queue.push({
        requestId: request_id,
        eventKind: 0,
        ok: 1,
        streamId: -1,
        payload: JSON.stringify({ devices: result }),
        error: "",
        code: ""
      });
    })
    .catch((error) => {
      if (media.cancelled.delete(request_id)) {
        return;
      }
      const failure = Module.neoloveMediaError(error);
      media.queue.push({
        requestId: request_id,
        eventKind: 0,
        ok: 0,
        streamId: -1,
        payload: "",
        error: failure.message,
        code: failure.code
      });
    });
  return 1;
});

EM_JS(int, neolove_js_media_request, (int request_id, const char* constraints_json), {
  const state = Module.neoloveState;
  const media = state && state.media;
  const devicesApi = navigator.mediaDevices;
  if (!media || !devicesApi || typeof devicesApi.getUserMedia !== "function" || !window.isSecureContext) {
    return 0;
  }

  let requested;
  try {
    requested = JSON.parse(UTF8ToString(constraints_json));
  } catch (error) {
    const failure = Module.neoloveMediaError(error);
    media.queue.push({
      requestId: request_id,
      eventKind: 1,
      ok: 0,
      streamId: -1,
      payload: "",
      error: failure.message,
      code: "invalid_options"
    });
    return 1;
  }

  const normalizeDeviceId = (value, prefix) => {
    if (typeof value !== "string" || !value) {
      return null;
    }
    const raw = value.startsWith(prefix) ? value.slice(prefix.length) : value;
    if (!raw || raw === "default" || raw.startsWith("anonymous-")) {
      return null;
    }
    return { exact: raw };
  };
  const audioConstraints = (value) => {
    if (!value || value === false) {
      return false;
    }
    const out = {};
    const deviceId = normalizeDeviceId(value.deviceId, "audio:");
    if (deviceId) out.deviceId = deviceId;
    if (Number.isFinite(value.sampleRate)) out.sampleRate = { ideal: value.sampleRate };
    if (Number.isFinite(value.channels)) out.channelCount = { ideal: value.channels };
    if (typeof value.echoCancellation === "boolean") out.echoCancellation = value.echoCancellation;
    if (typeof value.noiseSuppression === "boolean") out.noiseSuppression = value.noiseSuppression;
    if (typeof value.autoGainControl === "boolean") out.autoGainControl = value.autoGainControl;
    return Object.keys(out).length ? out : true;
  };
  const videoConstraints = (value) => {
    if (!value || value === false) {
      return false;
    }
    const out = {};
    const deviceId = normalizeDeviceId(value.deviceId, "camera:");
    if (deviceId) out.deviceId = deviceId;
    if (Number.isFinite(value.width)) out.width = { ideal: value.width };
    if (Number.isFinite(value.height)) out.height = { ideal: value.height };
    if (Number.isFinite(value.frameRate)) out.frameRate = { ideal: value.frameRate };
    if (typeof value.facingMode === "string" && value.facingMode) out.facingMode = { ideal: value.facingMode };
    return Object.keys(out).length ? out : true;
  };
  const constraints = {
    audio: audioConstraints(requested.audio),
    video: videoConstraints(requested.video)
  };

  devicesApi.getUserMedia(constraints)
    .then(async (stream) => {
      if (media.cancelled.delete(request_id)) {
        stream.getTracks().forEach((track) => track.stop());
        return;
      }
      const streamId = media.nextStreamId++;
      const entry = {
        stream,
        audio: null,
        video: null,
        lastError: "",
        stopped: false
      };
      const markEnded = (kind) => {
        entry.lastError = `${kind} track ended`;
      };
      stream.getTracks().forEach((track) => {
        track.addEventListener("ended", () => markEnded(track.kind), { once: true });
      });

      try {
        const audioTrack = stream.getAudioTracks()[0] || null;
        if (audioTrack) {
          const context = Module.neoloveEnsureAudioContext();
          if (context.state === "suspended") {
            try { await context.resume(); } catch (_error) {}
          }
          const settings = typeof audioTrack.getSettings === "function" ? audioTrack.getSettings() : {};
          const source = context.createMediaStreamSource(new MediaStream([audioTrack]));
          const requestedChannels = Math.max(1, settings.channelCount || 1);
          const processor = context.createScriptProcessor(2048, requestedChannels, 1);
          const silence = context.createGain();
          silence.gain.value = 0;
          const audio = {
            source,
            processor,
            silence,
            chunks: [],
            offset: 0,
            available: 0,
            dropped: 0,
            sampleRate: context.sampleRate || settings.sampleRate || 48000,
            channels: Math.max(1, settings.channelCount || 1),
            maxSamples: 0
          };
          audio.maxSamples = audio.sampleRate * audio.channels * 5;
          processor.onaudioprocess = (event) => {
            const input = event.inputBuffer;
            if (!input || input.numberOfChannels <= 0 || input.length <= 0) {
              return;
            }
            const channels = input.numberOfChannels;
            const frames = input.length;
            const sampleRate = input.sampleRate || context.sampleRate || audio.sampleRate;
            // Never mix two channel layouts or sample rates in one Luau
            // chunk. Browsers can renegotiate a live input track.
            if (audio.channels !== channels || audio.sampleRate !== sampleRate) {
              audio.dropped += audio.available;
              audio.chunks.length = 0;
              audio.offset = 0;
              audio.available = 0;
            }
            audio.channels = channels;
            audio.sampleRate = sampleRate;
            audio.maxSamples = audio.sampleRate * channels * 5;
            const interleaved = new Float32Array(frames * channels);
            for (let channel = 0; channel < channels; channel += 1) {
              const data = input.getChannelData(channel);
              for (let frame = 0; frame < frames; frame += 1) {
                interleaved[frame * channels + channel] = data[frame];
              }
            }
            audio.chunks.push(interleaved);
            audio.available += interleaved.length;
            let overflow = Math.max(0, audio.available - audio.maxSamples);
            audio.dropped += overflow;
            while (overflow > 0 && audio.chunks.length) {
              const first = audio.chunks[0];
              const remaining = first.length - audio.offset;
              const remove = Math.min(overflow, remaining);
              audio.offset += remove;
              audio.available -= remove;
              overflow -= remove;
              if (audio.offset >= first.length) {
                audio.chunks.shift();
                audio.offset = 0;
              }
            }
          };
          source.connect(processor);
          processor.connect(silence);
          silence.connect(context.destination);
          entry.audio = audio;
        }

        const videoTrack = stream.getVideoTracks()[0] || null;
        if (videoTrack) {
          const settings = typeof videoTrack.getSettings === "function" ? videoTrack.getSettings() : {};
          const element = document.createElement("video");
          element.muted = true;
          element.playsInline = true;
          element.autoplay = true;
          element.srcObject = new MediaStream([videoTrack]);
          try { await element.play(); } catch (_error) {}
          const frameCanvas = document.createElement("canvas");
          const frameContext = frameCanvas.getContext("2d", { willReadFrequently: true });
          if (!frameContext) {
            throw new Error("browser could not create a camera frame canvas");
          }
          const video = {
            element,
            canvas: frameCanvas,
            context: frameContext,
            settings,
            currentPixels: null,
            currentWidth: 0,
            currentHeight: 0,
            currentTimestamp: 0,
            currentDropped: 0,
            lastTime: -1,
            producedFrames: 0,
            consumedFrames: 0
          };
          if (typeof element.requestVideoFrameCallback === "function") {
            const countFrame = () => {
              if (entry.stopped) {
                return;
              }
              video.producedFrames += 1;
              element.requestVideoFrameCallback(countFrame);
            };
            element.requestVideoFrameCallback(countFrame);
          }
          entry.video = video;
        }

        // Permission prompts, AudioContext resume, and video.play() all await.
        // A Lua cancellation may arrive during any of them, so check again
        // before publishing the stream and dispose every partially built node.
        if (media.cancelled.delete(request_id)) {
          Module.neoloveDisposeMediaEntry(entry);
          return;
        }
        media.streams.set(streamId, entry);
        media.stoppedErrors.delete(streamId);
        if (entry.audio) media.permissions.microphone = "granted";
        if (entry.video) media.permissions.camera = "granted";
        const audioPayload = entry.audio ? {
          sampleRate: entry.audio.sampleRate,
          channels: entry.audio.channels
        } : null;
        const videoSettings = entry.video ? entry.video.settings : null;
        const videoPayload = entry.video ? {
          width: Number(videoSettings.width || entry.video.element.videoWidth || 0),
          height: Number(videoSettings.height || entry.video.element.videoHeight || 0),
          frameRate: Number(videoSettings.frameRate || 0)
        } : null;
        media.queue.push({
          requestId: request_id,
          eventKind: 1,
          ok: 1,
          streamId,
          payload: JSON.stringify({ audio: audioPayload, video: videoPayload }),
          error: "",
          code: ""
        });
      } catch (error) {
        Module.neoloveDisposeMediaEntry(entry);
        const failure = Module.neoloveMediaError(error);
        media.queue.push({
          requestId: request_id,
          eventKind: 1,
          ok: 0,
          streamId: -1,
          payload: "",
          error: failure.message,
          code: failure.code
        });
      }
    })
    .catch((error) => {
      if (media.cancelled.delete(request_id)) {
        return;
      }
      const failure = Module.neoloveMediaError(error);
      if (constraints.audio && failure.code === "permission_denied") media.permissions.microphone = "denied";
      if (constraints.video && failure.code === "permission_denied") media.permissions.camera = "denied";
      media.queue.push({
        requestId: request_id,
        eventKind: 1,
        ok: 0,
        streamId: -1,
        payload: "",
        error: failure.message,
        code: failure.code
      });
    });
  return 1;
});

EM_JS(void, neolove_js_media_cancel, (int request_id), {
  const media = Module.neoloveState && Module.neoloveState.media;
  if (media) {
    media.cancelled.add(request_id);
  }
});

EM_JS(int, neolove_js_media_poll, (int* request_id, int* event_kind, int* ok, int* stream_id), {
  const media = Module.neoloveState && Module.neoloveState.media;
  if (!media || !media.queue.length) {
    return 0;
  }
  const event = media.queue.shift();
  media.current = event;
  HEAP32[request_id >> 2] = event.requestId | 0;
  HEAP32[event_kind >> 2] = event.eventKind | 0;
  HEAP32[ok >> 2] = event.ok ? 1 : 0;
  HEAP32[stream_id >> 2] = event.streamId | 0;
  return 1;
});

EM_JS(int, neolove_js_media_copy_event_field, (int field, char* buffer, int capacity), {
  const media = Module.neoloveState && Module.neoloveState.media;
  const event = media && media.current;
  let value = "";
  if (event) {
    if (field === 0) value = event.payload || "";
    else if (field === 1) value = event.error || "";
    else if (field === 2) value = event.code || "";
  }
  const required = lengthBytesUTF8(value) + 1;
  if (capacity <= 0 || required > capacity) {
    return -required;
  }
  stringToUTF8(value, buffer, capacity);
  return required - 1;
});

EM_JS(int, neolove_js_media_permission, (int kind, char* buffer, int capacity), {
  const media = Module.neoloveState && Module.neoloveState.media;
  if (!media) {
    return 0;
  }
  const permissionName = kind === 1 ? "microphone" : kind === 2 ? "camera" : "";
  if (!permissionName) {
    return 0;
  }
  if (!navigator.mediaDevices || typeof navigator.mediaDevices.getUserMedia !== "function" || !window.isSecureContext) {
    media.permissions[permissionName] = "unavailable";
  } else if (!media.permissionQueries[permissionName]
      && navigator.permissions
      && typeof navigator.permissions.query === "function") {
    media.permissionQueries[permissionName] = true;
    try {
      navigator.permissions.query({ name: permissionName }).then((status) => {
        if (status && typeof status.state === "string") {
          media.permissions[permissionName] = status.state;
          status.onchange = () => { media.permissions[permissionName] = status.state; };
        }
      }).catch(() => {});
    } catch (_error) {
    }
  }
  const value = media.permissions[permissionName] || "prompt";
  const required = lengthBytesUTF8(value) + 1;
  if (capacity <= 0 || required > capacity) {
    return -required;
  }
  stringToUTF8(value, buffer, capacity);
  return required - 1;
});

EM_JS(int, neolove_js_media_supported, (int kind), {
  const supported = !!(window.isSecureContext
    && navigator.mediaDevices
    && typeof navigator.mediaDevices.getUserMedia === "function");
  return supported && (kind === 0 || kind === 1 || kind === 2) ? 1 : 0;
});

EM_JS(void, neolove_js_media_stop, (int stream_id), {
  if (Module.neoloveStopMediaStream) {
    Module.neoloveStopMediaStream(stream_id);
  }
});

EM_JS(void, neolove_js_media_stop_all, (), {
  const media = Module.neoloveState && Module.neoloveState.media;
  if (!media || !Module.neoloveStopMediaStream) {
    return;
  }
  Array.from(media.streams.keys()).forEach((id) => Module.neoloveStopMediaStream(id));
});

EM_JS(int, neolove_js_media_is_active, (int stream_id), {
  const media = Module.neoloveState && Module.neoloveState.media;
  const entry = media && media.streams.get(stream_id);
  return entry && entry.stream.active && entry.stream.getTracks().some((track) => track.readyState === "live") ? 1 : 0;
});

EM_JS(int, neolove_js_media_audio_info, (
  int stream_id,
  int* sample_rate,
  int* channels,
  int* available_samples,
  int* dropped_samples
), {
  const media = Module.neoloveState && Module.neoloveState.media;
  const entry = media && media.streams.get(stream_id);
  const audio = entry && entry.audio;
  if (!audio) {
    return 0;
  }
  HEAP32[sample_rate >> 2] = audio.sampleRate | 0;
  HEAP32[channels >> 2] = audio.channels | 0;
  HEAP32[available_samples >> 2] = audio.available | 0;
  HEAP32[dropped_samples >> 2] = audio.dropped | 0;
  return 1;
});

EM_JS(int, neolove_js_media_read_audio, (int stream_id, float* samples, int max_samples), {
  const media = Module.neoloveState && Module.neoloveState.media;
  const entry = media && media.streams.get(stream_id);
  const audio = entry && entry.audio;
  if (!audio || max_samples <= 0) {
    return 0;
  }
  const count = Math.min(max_samples, audio.available);
  let written = 0;
  while (written < count && audio.chunks.length) {
    const first = audio.chunks[0];
    const remaining = first.length - audio.offset;
    const copyCount = Math.min(count - written, remaining);
    HEAPF32.set(first.subarray(audio.offset, audio.offset + copyCount), (samples >> 2) + written);
    written += copyCount;
    audio.offset += copyCount;
    audio.available -= copyCount;
    if (audio.offset >= first.length) {
      audio.chunks.shift();
      audio.offset = 0;
    }
  }
  audio.dropped = 0;
  return written;
});

EM_JS(int, neolove_js_media_video_info, (
  int stream_id,
  int* width,
  int* height,
  double* timestamp,
  int* dropped_frames
), {
  const media = Module.neoloveState && Module.neoloveState.media;
  const entry = media && media.streams.get(stream_id);
  const video = entry && entry.video;
  if (!video) {
    return -1;
  }
  const element = video.element;
  const frameWidth = element.videoWidth | 0;
  const frameHeight = element.videoHeight | 0;
  if (element.readyState < 2 || frameWidth <= 0 || frameHeight <= 0 || element.currentTime === video.lastTime) {
    return 0;
  }
  try {
    video.lastTime = element.currentTime;
    if (video.canvas.width !== frameWidth || video.canvas.height !== frameHeight) {
      video.canvas.width = frameWidth;
      video.canvas.height = frameHeight;
    }
    video.context.drawImage(element, 0, 0, frameWidth, frameHeight);
    video.currentPixels = video.context.getImageData(0, 0, frameWidth, frameHeight).data;
    video.currentWidth = frameWidth;
    video.currentHeight = frameHeight;
    video.currentTimestamp = performance.now() / 1000.0;
    video.currentDropped = Math.max(0, video.producedFrames - video.consumedFrames - 1);
    video.consumedFrames = video.producedFrames;
    HEAP32[width >> 2] = frameWidth;
    HEAP32[height >> 2] = frameHeight;
    HEAPF64[timestamp >> 3] = video.currentTimestamp;
    HEAP32[dropped_frames >> 2] = video.currentDropped | 0;
    return video.currentPixels.length | 0;
  } catch (error) {
    entry.lastError = String((error && error.message) || error || "failed to read camera frame");
    return -2;
  }
});

EM_JS(int, neolove_js_media_copy_video, (int stream_id, uint8_t* pixels, int capacity), {
  const media = Module.neoloveState && Module.neoloveState.media;
  const entry = media && media.streams.get(stream_id);
  const video = entry && entry.video;
  if (!video || !video.currentPixels || capacity < video.currentPixels.length) {
    return 0;
  }
  HEAPU8.set(video.currentPixels, pixels);
  const written = video.currentPixels.length;
  video.currentPixels = null;
  return written;
});

EM_JS(int, neolove_js_media_copy_stream_error, (int stream_id, char* buffer, int capacity), {
  const media = Module.neoloveState && Module.neoloveState.media;
  const entry = media && media.streams.get(stream_id);
  const value = (entry && entry.lastError) || (media && media.stoppedErrors.get(stream_id)) || "";
  const required = lengthBytesUTF8(value) + 1;
  if (capacity <= 0 || required > capacity) {
    return -required;
  }
  stringToUTF8(value, buffer, capacity);
  return required - 1;
});

void neolove_web_bootstrap(void) {
  neolove_js_bootstrap();
}

double neolove_web_now_seconds(void) {
  return emscripten_get_now() / 1000.0;
}

int neolove_web_canvas_width(void) {
  return neolove_js_canvas_width();
}

int neolove_web_canvas_height(void) {
  return neolove_js_canvas_height();
}

double neolove_web_mouse_x(void) {
  return neolove_js_mouse_x();
}

double neolove_web_mouse_y(void) {
  return neolove_js_mouse_y();
}

int neolove_web_mouse_button_state(int index, int kind) {
  return neolove_js_mouse_button_state(index, kind);
}

double neolove_web_wheel_x(void) {
  return neolove_js_wheel_x();
}

double neolove_web_wheel_y(void) {
  return neolove_js_wheel_y();
}

int neolove_web_key_state(const char* name, int kind) {
  return neolove_js_key_state(name, kind);
}

int neolove_web_take_last_key(char* buffer, int capacity) {
  return neolove_js_take_last_key(buffer, capacity);
}

int neolove_web_take_char(char* buffer, int capacity) {
  return neolove_js_take_char(buffer, capacity);
}

void neolove_web_begin_frame(void) {
  neolove_js_begin_frame();
}

void neolove_web_clear_canvas(int r, int g, int b, int a) {
  neolove_js_clear_canvas(r, g, b, a);
}

void neolove_web_present_rgba(const uint8_t* pixels, int width, int height) {
  neolove_js_present_rgba(pixels, width, height);
}

int neolove_web_capture_rgba(uint8_t* pixels, int width, int height) {
  return neolove_js_capture_rgba(pixels, width, height);
}

void neolove_web_composite_rgba(const uint8_t* pixels, int width, int height, int x, int y) {
  neolove_js_composite_rgba(pixels, width, height, x, y);
}

void neolove_web_draw_image(
    uintptr_t image_id,
    double revision,
    const uint8_t* pixels,
    int image_width,
    int image_height,
    float source_x,
    float source_y,
    float source_w,
    float source_h,
    float dest_x,
    float dest_y,
    float dest_w,
    float dest_h,
    float rotation,
    float pivot_x,
    float pivot_y,
    float alpha,
    int linear_filter) {
  neolove_js_draw_image(
      image_id,
      revision,
      pixels,
      image_width,
      image_height,
      source_x,
      source_y,
      source_w,
      source_h,
      dest_x,
      dest_y,
      dest_w,
      dest_h,
      rotation,
      pivot_x,
      pivot_y,
      alpha,
      linear_filter);
}

void neolove_web_draw_shader(
    const char* fragment_source,
    const char* uniforms_json,
    const float* vertices,
    int vertex_count,
    uintptr_t texture_id,
    double texture_revision,
    const uint8_t* texture_pixels,
    int texture_width,
    int texture_height,
    int linear_filter,
    int antialiasing_mode,
    int depth_test) {
  neolove_js_draw_shader(
      fragment_source, uniforms_json, vertices, vertex_count, texture_id, texture_revision,
      texture_pixels, texture_width, texture_height, linear_filter, antialiasing_mode,
      depth_test);
}

void neolove_web_draw_text(
    const char* message,
    float x,
    float y,
    float w,
    float h,
    float rotation,
    float pivot_x,
    float pivot_y,
    int r,
    int g,
    int b,
    int a,
    float scale,
    float min_scale,
    int align_x,
    int align_y,
    int text_scale,
    int wrap,
    float padding_x,
    float padding_y,
    float line_spacing,
    float letter_spacing,
    int font_kind,
    const char* font_path) {
  neolove_js_draw_text(
      message,
      x,
      y,
      w,
      h,
      rotation,
      pivot_x,
      pivot_y,
      r,
      g,
      b,
      a,
      scale,
      min_scale,
      align_x,
      align_y,
      text_scale,
      wrap,
      padding_x,
      padding_y,
      line_spacing,
      letter_spacing,
      font_kind,
      font_path);
}

void neolove_web_report_status(const char* message) {
  neolove_js_report_status(message, 0);
}

void neolove_web_report_error(const char* message) {
  neolove_js_report_status(message, 1);
}

void neolove_web_debug_log(const char* message) {
  neolove_js_debug_log(message);
}

void neolove_web_mark_ready(void) {
  neolove_js_mark_ready();
}

int neolove_web_audio_play(
    int sound_id,
    const uint8_t* bytes,
    int bytes_len,
    int looped,
    float volume) {
  return neolove_js_audio_play(
      sound_id,
      bytes,
      bytes_len,
      looped,
      volume,
      0,
      0.0f,
      0.0f);
}

int neolove_web_audio_play_spatial(
    int sound_id,
    const uint8_t* bytes,
    int bytes_len,
    int looped,
    float volume,
    float x,
    float y) {
  return neolove_js_audio_play(
      sound_id, bytes, bytes_len, looped, volume, 1, x, y);
}

int neolove_web_audio_stop(int sound_id) {
  return neolove_js_audio_stop(sound_id);
}

int neolove_web_audio_set_volume(int sound_id, float volume) {
  return neolove_js_audio_set_volume(sound_id, volume);
}

int neolove_web_audio_set_position(int sound_id, float x, float y) {
  return neolove_js_audio_set_position(sound_id, x, y);
}

int neolove_web_audio_set_listener_position(float x, float y) {
  return neolove_js_audio_set_listener_position(x, y);
}

int neolove_web_audio_play_spatial_3d(
    int voice_id,
    const uint8_t* bytes,
    int bytes_len,
    int looped,
    float volume,
    float x,
    float y,
    float z,
    float min_distance,
    float max_distance,
    float rolloff,
    int distance_model) {
  return neolove_js_audio_play_spatial_3d(
      voice_id,
      bytes,
      bytes_len,
      looped,
      volume,
      x,
      y,
      z,
      min_distance,
      max_distance,
      rolloff,
      distance_model);
}

int neolove_web_audio_update_spatial_3d(
    int voice_id,
    float x,
    float y,
    float z,
    float volume,
    float min_distance,
    float max_distance,
    float rolloff,
    int distance_model) {
  return neolove_js_audio_update_spatial_3d(
      voice_id,
      x,
      y,
      z,
      volume,
      min_distance,
      max_distance,
      rolloff,
      distance_model);
}

int neolove_web_audio_stop_spatial_3d(int voice_id) {
  return neolove_js_audio_stop_spatial_3d(voice_id);
}

int neolove_web_audio_set_listener_3d(
    float x,
    float y,
    float z,
    float forward_x,
    float forward_y,
    float forward_z,
    float up_x,
    float up_y,
    float up_z) {
  return neolove_js_audio_set_listener_3d(
      x,
      y,
      z,
      forward_x,
      forward_y,
      forward_z,
      up_x,
      up_y,
      up_z);
}

int neolove_web_take_audio_error(char* buffer, int capacity) {
  return neolove_js_take_audio_error(buffer, capacity);
}

int neolove_web_http_start(
    int request_id,
    const char* url,
    const char* method,
    const char* headers_json,
    const uint8_t* body,
    int body_len) {
  return neolove_js_http_start(request_id, url, method, headers_json, body, body_len);
}

int neolove_web_http_poll(int* request_id, int* status, int* ok) {
  return neolove_js_http_poll(request_id, status, ok);
}

int neolove_web_http_copy_field(int field, char* buffer, int capacity) {
  return neolove_js_http_copy_field(field, buffer, capacity);
}

int neolove_web_media_enumerate(int request_id, int kind) {
  return neolove_js_media_enumerate(request_id, kind);
}

int neolove_web_media_request(int request_id, const char* constraints_json) {
  return neolove_js_media_request(request_id, constraints_json);
}

void neolove_web_media_cancel(int request_id) {
  neolove_js_media_cancel(request_id);
}

int neolove_web_media_poll(int* request_id, int* event_kind, int* ok, int* stream_id) {
  return neolove_js_media_poll(request_id, event_kind, ok, stream_id);
}

int neolove_web_media_copy_event_field(int field, char* buffer, int capacity) {
  return neolove_js_media_copy_event_field(field, buffer, capacity);
}

int neolove_web_media_permission(int kind, char* buffer, int capacity) {
  return neolove_js_media_permission(kind, buffer, capacity);
}

int neolove_web_media_supported(int kind) {
  return neolove_js_media_supported(kind);
}

void neolove_web_media_stop(int stream_id) {
  neolove_js_media_stop(stream_id);
}

void neolove_web_media_stop_all(void) {
  neolove_js_media_stop_all();
}

int neolove_web_media_is_active(int stream_id) {
  return neolove_js_media_is_active(stream_id);
}

int neolove_web_media_audio_info(
    int stream_id,
    int* sample_rate,
    int* channels,
    int* available_samples,
    int* dropped_samples) {
  return neolove_js_media_audio_info(
      stream_id, sample_rate, channels, available_samples, dropped_samples);
}

int neolove_web_media_read_audio(int stream_id, float* samples, int max_samples) {
  return neolove_js_media_read_audio(stream_id, samples, max_samples);
}

int neolove_web_media_video_info(
    int stream_id,
    int* width,
    int* height,
    double* timestamp,
    int* dropped_frames) {
  return neolove_js_media_video_info(stream_id, width, height, timestamp, dropped_frames);
}

int neolove_web_media_copy_video(int stream_id, uint8_t* pixels, int capacity) {
  return neolove_js_media_copy_video(stream_id, pixels, capacity);
}

int neolove_web_media_copy_stream_error(int stream_id, char* buffer, int capacity) {
  return neolove_js_media_copy_stream_error(stream_id, buffer, capacity);
}
