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
  state.statusEl = document.getElementById("status");
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
      existing.source.onended = null;
      existing.source.stop();
    } catch (_error) {
    }
    try {
      existing.source.disconnect();
    } catch (_error) {
    }
    try {
      existing.gain.disconnect();
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

  const syncCanvasSize = () => {
    const rect = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const width = Math.max(1, Math.round(Math.max(rect.width, 1) * dpr));
    const height = Math.max(1, Math.round(Math.max(rect.height, 1) * dpr));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    state.canvasWidth = canvas.width || width;
    state.canvasHeight = canvas.height || height;
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
      event.preventDefault();
    });

    canvas.addEventListener("mousedown", (event) => {
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
    state.initialized = true;
  }

  syncCanvasSize();

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
  const canvas = Module.canvas;
  if (!canvas) {
    return;
  }
  if (!state.ctx) {
    state.ctx = canvas.getContext("2d", { alpha: true, desynchronized: true });
    if (state.ctx) {
      state.ctx.imageSmoothingEnabled = false;
    }
  }
  if (!state.ctx) {
    return;
  }
  if (!state.imageData || state.imageData.width !== width || state.imageData.height !== height) {
    state.imageData = state.ctx.createImageData(width, height);
  }
  const view = HEAPU8.subarray(pixels, pixels + width * height * 4);
  state.imageData.data.set(view);
  state.ctx.putImageData(state.imageData, 0, 0);
});

EM_JS(void, neolove_js_report_status, (const char* message, int is_error), {
  const text = UTF8ToString(message);
  const state = Module.neoloveState;
  if (state && state.statusEl) {
    state.statusEl.textContent = text;
    state.statusEl.dataset.state = is_error ? "error" : "info";
  }
  if (is_error) {
    console.error(text);
  } else {
    console.log(text);
  }
});

EM_JS(void, neolove_js_mark_ready, (), {
  const state = Module.neoloveState;
  if (state && state.statusEl) {
    state.statusEl.textContent = "";
    state.statusEl.dataset.state = "ready";
  }
});

EM_JS(int, neolove_js_audio_play, (
  int sound_id,
  const float* samples,
  int samples_len,
  int sample_rate,
  int channels,
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

    if (channels <= 0) {
      throw new Error("sound must have at least one channel");
    }
    if (samples_len <= 0) {
      throw new Error("sound has no samples");
    }
    if (samples_len % channels !== 0) {
      throw new Error("sound sample buffer length must be a multiple of channels");
    }

    const frameCount = samples_len / channels;
    const sourceSamples = HEAPF32.subarray(samples >> 2, (samples >> 2) + samples_len);
    const audioBuffer = context.createBuffer(channels, frameCount, Math.max(1, sample_rate));
    for (let channel = 0; channel < channels; channel += 1) {
      const channelData = audioBuffer.getChannelData(channel);
      for (let frame = 0, index = channel; frame < frameCount; frame += 1, index += channels) {
        channelData[frame] = sourceSamples[index];
      }
    }

    const gain = context.createGain();
    gain.gain.value = Math.min(1, Math.max(0, volume));
    gain.connect(context.destination);

    const source = context.createBufferSource();
    source.buffer = audioBuffer;
    source.loop = !!looped;
    source.connect(gain);

    const entry = {
      source,
      gain,
      stopped: false
    };
    source.onended = () => {
      const current = state.audio.active.get(sound_id);
      if (current !== entry) {
        return;
      }
      state.audio.active.delete(sound_id);
      try {
        source.disconnect();
      } catch (_error) {
      }
      try {
        gain.disconnect();
      } catch (_error) {
      }
    };

    state.audio.active.set(sound_id, entry);
    if (context.state === "suspended") {
      const promise = context.resume();
      if (promise && typeof promise.catch === "function") {
        promise.catch(Module.neoloveSetAudioError);
      }
    }
    source.start(0);
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

void neolove_web_report_status(const char* message) {
  neolove_js_report_status(message, 0);
}

void neolove_web_report_error(const char* message) {
  neolove_js_report_status(message, 1);
}

void neolove_web_mark_ready(void) {
  neolove_js_mark_ready();
}

int neolove_web_audio_play(
    int sound_id,
    const float* samples,
    int samples_len,
    int sample_rate,
    int channels,
    int looped,
    float volume) {
  return neolove_js_audio_play(
      sound_id,
      samples,
      samples_len,
      sample_rate,
      channels,
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
