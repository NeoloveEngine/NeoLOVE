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
      lastError: "",
      resumeHooksInstalled: false
    }
  });

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
  if (Module.neoloveSyncCanvasSize) {
    Module.neoloveSyncCanvasSize();
  }
  return Module.neoloveState ? Module.neoloveState.canvasWidth : 1;
});

EM_JS(int, neolove_js_canvas_height, (), {
  if (Module.neoloveSyncCanvasSize) {
    Module.neoloveSyncCanvasSize();
  }
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
    state.ctx = canvas.getContext("2d", { alpha: false }) || canvas.getContext("2d");
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
  state.presentCount = (state.presentCount || 0) + 1;
  if (state.presentCount <= 5) {
    const centerPixel = Math.max(
      0,
      ((Math.floor(height / 2) * width + Math.floor(width / 2)) * 4)
    );
    console.warn("[NeoLOVE debug] present frame", {
      frame: state.presentCount,
      renderWidth: width,
      renderHeight: height,
      canvasWidth: canvas.width,
      canvasHeight: canvas.height,
      clientWidth: canvas.clientWidth,
      clientHeight: canvas.clientHeight,
      bytesExpected: expectedBytes,
      bytesReceived: view.length,
      firstPixel: Array.from(view.subarray(0, Math.min(4, view.length))),
      centerPixel: Array.from(view.subarray(centerPixel, Math.min(centerPixel + 4, view.length)))
    });
  }
  try {
    state.imageData.data.set(view);
    state.ctx.putImageData(state.imageData, 0, 0);
  } catch (error) {
    console.warn("[NeoLOVE debug] putImageData failed", error);
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
  int font_kind
), {
  const state = Module.neoloveState;
  if (!state || !Module.canvas) {
    return;
  }
  if (!state.ctx) {
    state.ctx = Module.canvas.getContext("2d", { alpha: false }) || Module.canvas.getContext("2d");
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
  const family = font_kind === 0 ? "monospace" : "sans-serif";
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
  float volume
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
        gain.gain.value = Math.min(1, Math.max(0, volume));
        gain.connect(context.destination);

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
      existing.gain.gain.value = Math.min(1, Math.max(0, volume));
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

void neolove_web_present_rgba(const uint8_t* pixels, int width, int height) {
  neolove_js_present_rgba(pixels, width, height);
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
    int font_kind) {
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
      font_kind);
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
      volume);
}

int neolove_web_audio_stop(int sound_id) {
  return neolove_js_audio_stop(sound_id);
}

int neolove_web_audio_set_volume(int sound_id, float volume) {
  return neolove_js_audio_set_volume(sound_id, volume);
}

int neolove_web_take_audio_error(char* buffer, int capacity) {
  return neolove_js_take_audio_error(buffer, capacity);
}
