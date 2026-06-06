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
    },
    fonts: new Map()
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
  try {
    state.imageData.data.set(view);
    state.ctx.putImageData(state.imageData, 0, 0);
  } catch (error) {
    console.warn("[NeoLOVE debug] putImageData failed", error);
  }
});


EM_JS(void, neolove_js_clear_canvas, (int r, int g, int b, int a), {
  const state = Module.neoloveState;
  const canvas = Module.canvas;
  if (!state || !canvas) {
    return;
  }
  if (!state.ctx) {
    state.ctx = canvas.getContext("2d", { alpha: false }) || canvas.getContext("2d");
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
    state.ctx = canvas.getContext("2d", { alpha: false }) || canvas.getContext("2d");
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

EM_JS(void, neolove_js_draw_shader_rect, (
  const char* fragment_source_ptr,
  const char* uniforms_json_ptr,
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
  int a
), {
  const state = Module.neoloveState;
  const canvas = Module.canvas;
  if (!state || !canvas || !(w > 0) || !(h > 0) || a <= 0) {
    return;
  }
  const fragmentSource = UTF8ToString(fragment_source_ptr);
  let uniforms = {};
  try {
    uniforms = JSON.parse(UTF8ToString(uniforms_json_ptr) || "{}");
  } catch (error) {
    console.error("NeoLOVE shader uniform JSON error", error);
  }

  const glCanvas = state.shaderCanvas || (state.shaderCanvas = document.createElement("canvas"));
  if (glCanvas.width !== canvas.width || glCanvas.height !== canvas.height) {
    glCanvas.width = canvas.width;
    glCanvas.height = canvas.height;
  }
  const gl = state.shaderGl || (state.shaderGl = glCanvas.getContext("webgl", { alpha: true, premultipliedAlpha: false }) || glCanvas.getContext("experimental-webgl"));
  if (!gl) {
    if (!state.shaderMissingWebglLogged) {
      state.shaderMissingWebglLogged = true;
      console.error("NeoLOVE web shaders require WebGL support in this browser");
    }
    return;
  }
  state.shaderPrograms = state.shaderPrograms || new Map();

  const vertexSource = `
attribute vec2 a_pos;
attribute vec2 a_uv;
attribute vec4 a_color;
varying lowp vec2 uv;
varying lowp vec4 color;
void main() {
  gl_Position = vec4(a_pos, 0.0, 1.0);
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

  let programInfo = state.shaderPrograms.get(fragmentSource);
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
      state.shaderPrograms.set(fragmentSource, programInfo);
    } catch (error) {
      console.error("NeoLOVE web shader compile failed", error);
      return;
    }
  }

  const toClipX = (px) => (px / Math.max(1, canvas.width)) * 2 - 1;
  const toClipY = (py) => 1 - (py / Math.max(1, canvas.height)) * 2;
  const rotate = (px, py) => {
    if (rotation === 0) {
      return [px, py];
    }
    const dx = px - pivot_x;
    const dy = py - pivot_y;
    const c = Math.cos(rotation);
    const s = Math.sin(rotation);
    return [pivot_x + dx * c - dy * s, pivot_y + dx * s + dy * c];
  };
  const corners = [
    rotate(x, y),
    rotate(x + w, y),
    rotate(x + w, y + h),
    rotate(x, y + h)
  ];
  const color = [r / 255, g / 255, b / 255, a / 255];
  const packed = new Float32Array([
    toClipX(corners[0][0]), toClipY(corners[0][1]), 0, 0, color[0], color[1], color[2], color[3],
    toClipX(corners[1][0]), toClipY(corners[1][1]), 1, 0, color[0], color[1], color[2], color[3],
    toClipX(corners[2][0]), toClipY(corners[2][1]), 1, 1, color[0], color[1], color[2], color[3],
    toClipX(corners[0][0]), toClipY(corners[0][1]), 0, 0, color[0], color[1], color[2], color[3],
    toClipX(corners[2][0]), toClipY(corners[2][1]), 1, 1, color[0], color[1], color[2], color[3],
    toClipX(corners[3][0]), toClipY(corners[3][1]), 0, 1, color[0], color[1], color[2], color[3]
  ]);

  const buffer = state.shaderVertexBuffer || (state.shaderVertexBuffer = gl.createBuffer());
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, packed, gl.STREAM_DRAW);
  gl.viewport(0, 0, glCanvas.width, glCanvas.height);
  gl.disable(gl.DEPTH_TEST);
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
  gl.clearColor(0, 0, 0, 0);
  gl.clear(gl.COLOR_BUFFER_BIT);
  gl.useProgram(programInfo.program);

  const stride = 8 * 4;
  if (programInfo.aPos >= 0) {
    gl.enableVertexAttribArray(programInfo.aPos);
    gl.vertexAttribPointer(programInfo.aPos, 2, gl.FLOAT, false, stride, 0);
  }
  if (programInfo.aUv >= 0) {
    gl.enableVertexAttribArray(programInfo.aUv);
    gl.vertexAttribPointer(programInfo.aUv, 2, gl.FLOAT, false, stride, 2 * 4);
  }
  if (programInfo.aColor >= 0) {
    gl.enableVertexAttribArray(programInfo.aColor);
    gl.vertexAttribPointer(programInfo.aColor, 4, gl.FLOAT, false, stride, 4 * 4);
  }

  if (!state.shaderWhiteTexture) {
    state.shaderWhiteTexture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, state.shaderWhiteTexture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([255, 255, 255, 255]));
  }
  gl.activeTexture(gl.TEXTURE0);
  gl.bindTexture(gl.TEXTURE_2D, state.shaderWhiteTexture);
  if (programInfo.uTexture !== null) {
    gl.uniform1i(programInfo.uTexture, 0);
  }

  const floats = uniforms.floats || {};
  for (const name of Object.keys(floats)) {
    let location = programInfo.uniformLocations.get(name);
    if (location === undefined) {
      location = gl.getUniformLocation(programInfo.program, name);
      programInfo.uniformLocations.set(name, location);
    }
    if (!location) {
      continue;
    }
    const values = floats[name] || [];
    if (values.length === 1) gl.uniform1f(location, values[0]);
    else if (values.length === 2) gl.uniform2f(location, values[0], values[1]);
    else if (values.length === 3) gl.uniform3f(location, values[0], values[1], values[2]);
    else if (values.length >= 4) gl.uniform4f(location, values[0], values[1], values[2], values[3]);
  }

  gl.drawArrays(gl.TRIANGLES, 0, 6);

  if (!state.ctx) {
    state.ctx = canvas.getContext("2d", { alpha: false }) || canvas.getContext("2d");
  }
  if (state.ctx) {
    state.ctx.save();
    state.ctx.setTransform(1, 0, 0, 1, 0, 0);
    const sx = Math.max(0, Math.floor(Math.min(corners[0][0], corners[1][0], corners[2][0], corners[3][0])));
    const sy = Math.max(0, Math.floor(Math.min(corners[0][1], corners[1][1], corners[2][1], corners[3][1])));
    const ex = Math.min(canvas.width, Math.ceil(Math.max(corners[0][0], corners[1][0], corners[2][0], corners[3][0])));
    const ey = Math.min(canvas.height, Math.ceil(Math.max(corners[0][1], corners[1][1], corners[2][1], corners[3][1])));
    state.ctx.globalCompositeOperation = "source-over";
    if (ex > sx && ey > sy) {
      state.ctx.drawImage(glCanvas, sx, sy, ex - sx, ey - sy, sx, sy, ex - sx, ey - sy);
    }
    state.ctx.restore();
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
  const fontPath = font_path_ptr ? UTF8ToString(font_path_ptr) : "";
  const quoteFontFamily = (family) => `"${String(family).replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
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
  const family = customFont && customFont.loaded ? quoteFontFamily(customFont.family) :
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

void neolove_web_clear_canvas(int r, int g, int b, int a) {
  neolove_js_clear_canvas(r, g, b, a);
}

void neolove_web_present_rgba(const uint8_t* pixels, int width, int height) {
  neolove_js_present_rgba(pixels, width, height);
}

void neolove_web_composite_rgba(const uint8_t* pixels, int width, int height, int x, int y) {
  neolove_js_composite_rgba(pixels, width, height, x, y);
}

void neolove_web_draw_shader_rect(
    const char* fragment_source,
    const char* uniforms_json,
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
    int a) {
  neolove_js_draw_shader_rect(
      fragment_source,
      uniforms_json,
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
      a);
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
