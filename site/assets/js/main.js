(function () {
  "use strict";

  var prefersReduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var shaderMix = 0;

  /* ---------- translations ---------- */
  var I18N = {
    en: {
      "meta.title": "MediaA — screenshots, screen & audio recording, conversion. Free.",
      "meta.description": "MediaA is a free Windows tool for annotated screenshots, screen and audio recording, and conversion between 17+ formats. One file, no cloud.",
      "og.title": "MediaA — snap, record, convert",
      "og.description": "Screenshots, screen and audio recording, format conversion. Free and open source.",
      "nav.capture": "Capture",
      "nav.record": "Record",
      "nav.convert": "Convert",
      "nav.why": "Why MediaA",
      "hero.eyebrow": "Free · Open source · Windows",
      "hero.line1": "Snap. Record.",
      "hero.line2": "Convert.",
      "hero.sub": "MediaA is one tool that takes annotated screenshots, captures your screen and sound, and instantly turns the result into the format you need. No subscriptions, no accounts, and none of your files ever leave your computer.",
      "hero.cta.primary": "See what it can do",
      "hero.cta.ghost": "Download free ↗",
      "hero.stat.free": "$0",
      "hero.stat.free.label": "forever",
      "hero.stat.formats.unit": "+",
      "hero.stat.formats.label": "formats",
      "hero.stat.fps.unit": " fps",
      "hero.stat.fps.label": "recording",
      "hero.stat.os": "Windows",
      "hero.stat.os.label": "supported",
      "camera.shutter": "Take a photo",
      "camera.hidePeek": "Hide camera",
      "hero.device.title": "MediaA — Record",
      "hero.chip2.label": "animation",
      "hero.chip3": "Mic + system audio",
      "hero.chip3.label": "audio",
      "alt.record": "MediaA recording window",
      "alt.record2": "MediaA recording screen: video of the screen, an area or windows",
      "alt.capture": "MediaA capture screen: screenshots of the screen, area and windows",
      "alt.convert": "MediaA conversion screen: change format, quality and resolution",
      "alt.region": "Selecting a screen area with a frame and its size",
      "alt.annotate": "Annotation palette: brush, arrow, rectangle, text",
      "alt.audio": "Audio source settings: microphone and system sound",
      "alt.settings": "MediaA settings: theme, hotkeys, language",
      "cap.kicker": "Three moves and you're done",
      "cap.h2": "All the magic in three buttons",
      "cap.lede": "No half-screen timelines or a hundred settings. Capture it, record it, convert it.",
      "cap.pillar1.h3": "Capture",
      "cap.pillar1.p": "A snapshot of an area, a window or the whole screen. Drag a frame, nudge it with the arrow keys, and draw right on top: arrows, text, brush, shapes.",
      "cap.pillar1.li1": "Lightshot-style area picker",
      "cap.pillar1.li2": "Annotations with undo",
      "cap.pillar1.li3": "Multiple monitors",
      "cap.pillar1.li4": "PNG and WebP",
      "cap.pillar2.h3": "Record",
      "cap.pillar2.p": "Video of the screen, an area, or several windows at once — each to its own file. Add your voice and system audio, pause, and stop right from the tray.",
      "cap.pillar2.li1": "MP4 / MKV or GIF / WebP animation",
      "cap.pillar2.li2": "Microphone and system sound",
      "cap.pillar2.li3": "Pause and stop from the tray",
      "cap.pillar2.li4": "Up to 60 frames per second",
      "cap.pillar3.h3": "Convert",
      "cap.pillar3.p": "Turn anything into anything: video to GIF, WebP to MP4, an image to WebP, audio to MP3. Quality, resolution and bitrate are all yours.",
      "cap.pillar3.li1": "17+ formats both ways",
      "cap.pillar3.li2": "Drop the audio track",
      "cap.pillar3.li3": "Quality and bitrate control",
      "cap.pillar3.li4": "Progress and cancel",
      "spot.kicker": "Details that hook you",
      "spot.h2": "Recording without the fuss",
      "spot.lede": "The things that usually hide behind three menus just work from the first click here.",
      "spot.region.h3": "A frame that obeys",
      "spot.region.p": "Freeze the screen, select an area and refine it to the pixel — resize handles, keyboard nudging and the size right on the frame.",
      "spot.annotate.h3": "Draw on the fly",
      "spot.annotate.p": "Brush, line, arrow, rectangle, highlighter, text and color — everything you need to explain a screenshot without words.",
      "spot.audio.h3": "Voice and system",
      "spot.audio.p": "Record narration, app audio, or both at once — MP3, M4A, WAV, FLAC or OGG.",
      "spot.settings.h3": "Make it yours",
      "spot.settings.p": "Global hotkeys, dark and light themes, English and Russian — everything is remembered and waiting on the next launch.",
      "cnv.kicker": "Conversion is magic too",
      "cnv.h2": "From one format to any",
      "cnv.lede": "FFmpeg is already inside, so there is nothing to install: MediaA knows its formats by heart.",
      "cnv.step1.h3": "Pick a file",
      "cnv.step1.p": "Video, animation, image or audio — from your file manager or in one click.",
      "cnv.step2.h3": "Set the target",
      "cnv.step2.p": "Format, quality, resolution, bitrate. Want silence? Drop the audio from a video.",
      "cnv.step3.h3": "Hit Convert",
      "cnv.step3.p": "Watch the progress, cancel anytime, and grab the finished file.",
      "why.kicker": "Why MediaA",
      "why.h2": "A tool that respects you",
      "why.free.h3": "Free forever",
      "why.free.p": "No subscriptions, watermarks or recording limits. The source is open — look for yourself.",
      "why.one.h3": "One file",
      "why.one.p": "A single executable with FFmpeg built in. Nothing to install or configure.",
      "why.local.h3": "Everything stays yours",
      "why.local.p": "Files are written to your disk. No clouds, no accounts, none of your recordings sent anywhere.",
      "why.hotkeys.h3": "Global hotkeys",
      "why.hotkeys.p": "Snap an area or start recording in one press. Tune the shortcuts to your muscle memory.",
      "why.crash.h3": "Crash-proof",
      "why.crash.p": "Recording cut off by a shutdown? MediaA restores it intact on the next launch.",
      "why.theme.h3": "Dark and light",
      "why.theme.p": "Switch themes to match your mood or the time of day. The language follows your system.",
      "cta.lead": "Start capturing beautifully.",
      "cta.free": "Free.",
      "cta.p": "Download MediaA, select an area — and a minute later you'll have your first annotated screenshot.",
      "cta.primary": "Download ↗",
      "cta.ghost": "Back to top",
      "footer.p": "Windows screen capture in Rust. Screenshots, screen and audio recording, conversion.",
      "footer.source": "Source code",
      "footer.build": "Build from source",
      "footer.note": "MIT © MediaA. Binary builds are GPL due to FFmpeg/libx264."
    },
    ru: {
      "meta.title": "MediaA — скриншоты, запись экрана и звука, конвертация. Бесплатно.",
      "meta.description": "MediaA — бесплатный инструмент для Windows: скриншоты с аннотациями, запись экрана и звука, конвертация между 17+ форматами. Один файл, без облаков.",
      "og.title": "MediaA — снимай, записывай, конвертируй",
      "og.description": "Скриншоты, запись экрана и звука, конвертация форматов. Бесплатно, с открытым исходным кодом.",
      "nav.capture": "Захват",
      "nav.record": "Запись",
      "nav.convert": "Конвертация",
      "nav.why": "Почему MediaA",
      "hero.eyebrow": "Бесплатно · Открытый исходный код · Windows",
      "hero.line1": "Снимай. Записывай.",
      "hero.line2": "Конвертируй.",
      "hero.sub": "MediaA — один инструмент, который делает скриншоты с аннотациями, пишет экран и звук и мгновенно превращает результат в нужный формат. Без подписок, без аккаунтов и без отправки твоих файлов в облако.",
      "hero.cta.primary": "Смотреть возможности",
      "hero.cta.ghost": "Скачать бесплатно ↗",
      "hero.stat.free": "0 ₽",
      "hero.stat.free.label": "навсегда",
      "hero.stat.formats.unit": "+",
      "hero.stat.formats.label": "форматов",
      "hero.stat.fps.unit": " fps",
      "hero.stat.fps.label": "запись",
      "hero.stat.os": "Windows",
      "hero.stat.os.label": "поддерживается",
      "camera.shutter": "Сделать снимок",
      "camera.hidePeek": "Спрятать камеру",
      "hero.device.title": "MediaA — Запись",
      "hero.chip2.label": "анимация",
      "hero.chip3": "Микрофон + системный звук",
      "hero.chip3.label": "звук",
      "alt.record": "Окно записи экрана MediaA",
      "alt.record2": "Экран записи MediaA: видео с экрана, области или окон",
      "alt.capture": "Экран захвата MediaA: скриншоты экрана, области и окон",
      "alt.convert": "Экран конвертации MediaA: смена формата, качества и разрешения",
      "alt.region": "Выделение области экрана с рамкой и размерами",
      "alt.annotate": "Палитра аннотаций: кисть, стрелка, прямоугольник, текст",
      "alt.audio": "Настройка источников звука: микрофон и системный звук",
      "alt.settings": "Настройки MediaA: тема, горячие клавиши, язык",
      "cap.kicker": "Три действия — и готово",
      "cap.h2": "Вся магия в трёх кнопках",
      "cap.lede": "Никаких таймлайнов на пол-экрана и сотни настроек. Захватил, записал, перевёл в нужный формат.",
      "cap.pillar1.h3": "Захват",
      "cap.pillar1.p": "Снимок области, окна или всего экрана. Выделяй рамку, двигай её стрелками и рисуй прямо поверх: стрелки, текст, кисть, фигуры.",
      "cap.pillar1.li1": "Выбор области как в Lightshot",
      "cap.pillar1.li2": "Аннотации с отменой действий",
      "cap.pillar1.li3": "Несколько мониторов",
      "cap.pillar1.li4": "PNG и WebP",
      "cap.pillar2.h3": "Запись",
      "cap.pillar2.p": "Видео с экрана, области или сразу нескольких окон — каждое в свой файл. Добавь голос и звук системы, поставь паузу и остановись прямо из трея.",
      "cap.pillar2.li1": "MP4 / MKV или анимация GIF / WebP",
      "cap.pillar2.li2": "Микрофон и системный звук",
      "cap.pillar2.li3": "Пауза и остановка из трея",
      "cap.pillar2.li4": "До 60 кадров в секунду",
      "cap.pillar3.h3": "Конвертация",
      "cap.pillar3.p": "Переведи что угодно во что угодно: видео в GIF, WebP в MP4, картинку в WebP, звук в MP3. Качество, разрешение и битрейт — под тобой.",
      "cap.pillar3.li1": "17+ форматов в обе стороны",
      "cap.pillar3.li2": "Убрать звуковую дорожку",
      "cap.pillar3.li3": "Настройка качества и битрейта",
      "cap.pillar3.li4": "Прогресс и отмена",
      "spot.kicker": "Детали, которые цепляют",
      "spot.h2": "Запись без лишних движений",
      "spot.lede": "То, что обычно прячут за тремя меню, здесь работает с первого клика.",
      "spot.region.h3": "Рамка, которая слушается",
      "spot.region.p": "Заморозь экран, выдели область и доведи её до пикселя — ручки изменения, подгонка клавишами и размер прямо на рамке.",
      "spot.annotate.h3": "Рисуй на лету",
      "spot.annotate.p": "Кисть, линия, стрелка, прямоугольник, выделение, текст и цвет — всё, чтобы объяснить скриншот без слов.",
      "spot.audio.h3": "Голос и система",
      "spot.audio.p": "Пиши озвучку, звук приложения или оба сразу — MP3, M4A, WAV, FLAC или OGG.",
      "spot.settings.h3": "Настрой под себя",
      "spot.settings.p": "Глобальные горячие клавиши, тёмная и светлая темы, английский и русский — всё запоминается и ждёт тебя при следующем запуске.",
      "cnv.kicker": "Конвертация — тоже магия",
      "cnv.h2": "Из одного формата — в любой",
      "cnv.lede": "FFmpeg уже внутри, поэтому ничего ставить не нужно: MediaA знает свои форматы наизусть.",
      "cnv.step1.h3": "Выбери файл",
      "cnv.step1.p": "Видео, анимацию, картинку или звук — из проводника или одним кликом.",
      "cnv.step2.h3": "Задай цель",
      "cnv.step2.p": "Формат, качество, разрешение, битрейт. Хочешь — убери звук из видео.",
      "cnv.step3.h3": "Нажми «Конвертировать»",
      "cnv.step3.p": "Следи за прогрессом, отменяй в любой момент и забирай готовый файл.",
      "why.kicker": "Почему MediaA",
      "why.h2": "Инструмент, который уважает тебя",
      "why.free.h3": "Бесплатно навсегда",
      "why.free.p": "Никаких подписок, водяных знаков и лимитов на запись. Исходный код открыт — загляни и убедись сам.",
      "why.one.h3": "Один файл",
      "why.one.p": "Один исполняемый файл со встроенным FFmpeg. Ничего не доустанавливай и не настраивай.",
      "why.local.h3": "Всё остаётся у тебя",
      "why.local.p": "Файлы пишутся на твой диск. Никаких облаков, аккаунтов и отправки твоих записей куда-либо.",
      "why.hotkeys.h3": "Горячие клавиши",
      "why.hotkeys.p": "Снимок области и старт записи — в один нажим. Настрой сочетания под свою мышечную память.",
      "why.crash.h3": "Не боится сбоев",
      "why.crash.p": "Оборвалась запись из-за выключения? MediaA восстановит её в целости при следующем запуске.",
      "why.theme.h3": "Тёмная и светлая",
      "why.theme.p": "Переключай тему под настроение или время суток. Язык подхватывается из системы.",
      "cta.lead": "Начни снимать красиво.",
      "cta.free": "Бесплатно.",
      "cta.p": "Скачай MediaA, выдели область — и через минуту у тебя будет первый скриншот с подписью.",
      "cta.primary": "Скачать ↗",
      "cta.ghost": "Вернуться наверх",
      "footer.p": "Инструмент захвата экрана на Rust для Windows. Скриншоты, запись экрана и звука, конвертация.",
      "footer.source": "Исходный код",
      "footer.build": "Сборка из исходников",
      "footer.note": "MIT © MediaA. Бинарные сборки под GPL из-за FFmpeg/libx264."
    }
  };

  var currentLang = "en";

  function t(key) {
    var table = I18N[currentLang] || I18N.en;
    return table[key] !== undefined ? table[key] : key;
  }

  function detectLang() {
    try {
      var saved = localStorage.getItem("mediaa-lang");
      if (saved && I18N[saved]) return saved;
    } catch (e) {}
    var nav = (navigator.language || "en").toLowerCase();
    return nav.indexOf("ru") === 0 ? "ru" : "en";
  }

  function wobbleHtml(text) {
    var units = [];
    var punctuation = ".!?…";
    for (var i = 0; i < text.length; i++) {
      var ch = text[i];
      if (punctuation.indexOf(ch) !== -1 && units.length) {
        units[units.length - 1] += ch;
      } else {
        units.push(ch);
      }
    }
    var html = "";
    units.forEach(function (unit, index) {
      html += '<span class="wobble-letter" style="--i:' + index + '">' + unit.replace(/ /g, "&nbsp;") + "</span>";
    });
    return html;
  }

  function applyWobble(el) {
    if (prefersReduced) {
      el.textContent = el.textContent;
      el.classList.remove("is-wobble");
      return;
    }
    el.innerHTML = wobbleHtml(el.textContent);
    el.classList.add("is-wobble");
  }

  function scrambleTo(el, finalText) {
    el.classList.remove("is-wobble");
    if (!finalText || prefersReduced) {
      el.textContent = finalText;
      return;
    }
    var chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZАБВГДЕЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯ0123456789";
    var frame = 0;
    var total = 28;
    var tick = function () {
      frame++;
      var progress = frame / total;
      var out = "";
      for (var i = 0; i < finalText.length; i++) {
        var c = finalText[i];
        if (c === " " || c === ".") {
          out += c;
        } else if (i / finalText.length < progress) {
          out += c;
        } else {
          out += chars[Math.floor(Math.random() * chars.length)];
        }
      }
      el.textContent = out;
      if (frame < total) {
        requestAnimationFrame(tick);
      } else {
        el.innerHTML = wobbleHtml(finalText);
        el.classList.add("is-wobble");
      }
    };
    requestAnimationFrame(tick);
  }

  function setLang(lang) {
    if (!I18N[lang]) lang = "en";
    currentLang = lang;
    document.documentElement.lang = lang;

    document.querySelectorAll("[data-i18n]").forEach(function (el) {
      var key = el.getAttribute("data-i18n");
      var val = t(key);
      if (val !== key) {
        el.textContent = val;
        if (el.hasAttribute("data-scramble")) el.classList.remove("is-wobble");
      }
    });
    document.querySelectorAll("[data-i18n-content]").forEach(function (el) {
      var key = el.getAttribute("data-i18n-content");
      var val = t(key);
      if (val !== key) el.setAttribute("content", val);
    });
    document.querySelectorAll("[data-i18n-alt]").forEach(function (el) {
      var key = el.getAttribute("data-i18n-alt");
      var val = t(key);
      if (val !== key) el.setAttribute("alt", val);
    });
    document.querySelectorAll("[data-i18n-aria-label]").forEach(function (el) {
      var key = el.getAttribute("data-i18n-aria-label");
      var val = t(key);
      if (val !== key) el.setAttribute("aria-label", val);
    });
    document.querySelectorAll("[data-wobble]").forEach(function (el) {
      applyWobble(el);
    });
    document.querySelectorAll(".lang-btn").forEach(function (btn) {
      btn.classList.toggle("active", btn.getAttribute("data-lang") === lang);
    });

    try {
      localStorage.setItem("mediaa-lang", lang);
    } catch (e) {}

    document.querySelectorAll("[data-scramble].in").forEach(function (el) {
      scrambleTo(el, t(el.getAttribute("data-i18n")));
    });
  }

  /* ---------- WebGL aurora background ---------- */
  function initShader() {
    var canvas = document.getElementById("shader");
    if (!canvas) return;

    var gl = canvas.getContext("webgl", { alpha: false, antialias: false, powerPreference: "high-performance" });
    if (!gl) return;

    var vertex = [
      "attribute vec2 a_pos;",
      "void main(){ gl_Position = vec4(a_pos, 0.0, 1.0); }"
    ].join("\n");

    var fragment = [
      "precision highp float;",
      "uniform vec2 u_res;",
      "uniform float u_time;",
      "uniform float u_mix;",
      "uniform float u_scroll;",
      "float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123); }",
      "float noise(vec2 p){",
      "  vec2 i = floor(p); vec2 f = fract(p);",
      "  f = f * f * (3.0 - 2.0 * f);",
      "  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x),",
      "             mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);",
      "}",
      "float fbm(vec2 p){",
      "  float v = 0.0; float a = 0.5;",
      "  for (int i = 0; i < 5; i++){ v += a * noise(p); p *= 2.02; a *= 0.5; }",
      "  return v;",
      "}",
      "float starLayer(vec2 p, float scale, float seed, float threshold){",
      "  vec2 sp = p * scale + seed;",
      "  vec2 id = floor(sp);",
      "  vec2 gv = fract(sp) - 0.5;",
      "  float h = hash(id + seed);",
      "  vec2 pos = (vec2(hash(id + 3.1), hash(id + 7.7)) - 0.5) * 0.72;",
      "  float d = length(gv - pos);",
      "  float core = smoothstep(0.055, 0.0, d);",
      "  float glow = smoothstep(0.16, 0.0, d) * 0.22;",
      "  float twinkle = 0.65 + 0.35 * sin(u_time * (0.7 + h * 1.8) + h * 6.28318);",
      "  return (core + glow) * step(threshold, h) * twinkle;",
      "}",
      "void main(){",
      "  vec2 uv = gl_FragCoord.xy / u_res.xy;",
      "  vec2 p = (gl_FragCoord.xy - 0.5 * u_res.xy) / u_res.y;",
      "  float t = u_time * 0.06;",
      "  vec2 q = vec2(fbm(p + vec2(0.0, 0.0) + t), fbm(p + vec2(5.2, 1.3) - t));",
      "  vec2 r = vec2(fbm(p + 2.0 * q + vec2(1.7, 9.2) + 0.15 * t), fbm(p + 2.0 * q + vec2(8.3, 2.8) - 0.12 * t));",
      "  float f = fbm(p + 1.9 * r);",
      "  vec3 deep = mix(vec3(0.02, 0.06, 0.20), vec3(0.12, 0.04, 0.24), u_mix);",
      "  vec3 blue = mix(vec3(0.10, 0.35, 0.72), vec3(0.46, 0.18, 0.84), u_mix);",
      "  vec3 cyan = mix(vec3(0.08, 0.70, 0.80), vec3(0.68, 0.42, 0.96), u_mix);",
      "  vec3 col = mix(deep, blue, smoothstep(0.0, 0.7, f));",
      "  col = mix(col, cyan, smoothstep(0.45, 1.0, f) * 0.7);",
      "  float side = smoothstep(0.28, 0.95, abs(uv.x - 0.5) * 2.0);",
      "  col *= mix(0.07, 1.0, side);",
      "  col += (1.0 - side) * vec3(0.16, 0.30, 0.46) * 0.07;",
      "  float vertical = smoothstep(0.0, 0.12, uv.y) * smoothstep(0.0, 0.12, 1.0 - uv.y);",
      "  col *= mix(0.55, 1.0, vertical);",
      "  float scroll = u_scroll;",
      "  float starsA = starLayer(p + vec2(0.0, -scroll * 0.07), 16.0, 3.0, 0.88);",
      "  float starsB = starLayer(p + vec2(4.7, -scroll * 0.13), 27.0, 11.0, 0.93) * 0.65;",
      "  float starsWarm = starLayer(p + vec2(2.4, -scroll * 0.1), 38.0, 19.0, 0.8) * 0.7;",
      "  float starsViolet = starLayer(p + vec2(6.8, -scroll * 0.12), 41.0, 23.0, 0.86) * 0.55;",
      "  float fog = smoothstep(0.25, 0.7, f);",
      "  float starMask = mix(0.9, 1.0, side) * (0.6 + 0.4 * fog);",
      "  float centerStarMask = (1.0 - side) * (0.45 + 0.55 * fog);",
      "  col += (starsA + starsB) * vec3(0.62, 0.82, 1.0) * starMask * 0.7;",
      "  col += starsWarm * vec3(1.0, 0.6, 0.15) * centerStarMask * 0.62;",
      "  col += starsViolet * vec3(0.72, 0.5, 1.0) * centerStarMask * 0.45;",
      "  col += (hash(gl_FragCoord.xy + u_time) - 0.5) * mix(0.004, 0.02, side);",
      "  gl_FragColor = vec4(col, 1.0);",
      "}"
    ].join("\n");

    function compile(type, src) {
      var sh = gl.createShader(type);
      gl.shaderSource(sh, src);
      gl.compileShader(sh);
      if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
        gl.deleteShader(sh);
        return null;
      }
      return sh;
    }

    var vs = compile(gl.VERTEX_SHADER, vertex);
    var fs = compile(gl.FRAGMENT_SHADER, fragment);
    if (!vs || !fs) return;

    var prog = gl.createProgram();
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return;
    gl.useProgram(prog);

    var buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    var loc = gl.getAttribLocation(prog, "a_pos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

    var uRes = gl.getUniformLocation(prog, "u_res");
    var uTime = gl.getUniformLocation(prog, "u_time");
    var uMix = gl.getUniformLocation(prog, "u_mix");
    var uScroll = gl.getUniformLocation(prog, "u_scroll");
    function resize() {
      var dpr = Math.min(window.devicePixelRatio || 1, 2);
      var w = Math.floor(canvas.clientWidth * dpr);
      var h = Math.floor(canvas.clientHeight * dpr);
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
        gl.viewport(0, 0, w, h);
      }
    }
    resize();
    window.addEventListener("resize", resize);

    var start = performance.now();
    function frame(now) {
      gl.uniform2f(uRes, canvas.width, canvas.height);
      gl.uniform1f(uTime, (now - start) / 1000);
      gl.uniform1f(uMix, shaderMix);
      gl.uniform1f(uScroll, window.scrollY / Math.max(1, window.innerHeight));
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
      requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);
  }

  /* ---------- CRT shader for the hero screenshot ---------- */
  function initCrt() {
    var img = document.querySelector(".crt-fallback");
    var canvas = document.querySelector(".crt-canvas");
    if (!img || !canvas) return;

    var gl = canvas.getContext("webgl", { antialias: false, alpha: false });
    if (!gl) return;

    var vertex = [
      "attribute vec2 a_pos;",
      "varying vec2 v_uv;",
      "void main(){ v_uv = a_pos * 0.5 + 0.5; gl_Position = vec4(a_pos, 0.0, 1.0); }"
    ].join("\n");

    var fragment = [
      "precision highp float;",
      "uniform sampler2D u_tex;",
      "uniform float u_time;",
      "varying vec2 v_uv;",
      "float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123); }",
      "vec2 curve(vec2 uv){",
      "  uv = uv * 2.0 - 1.0;",
      "  vec2 off = abs(uv.yx) / vec2(5.5, 4.5);",
      "  uv += uv * off * off;",
      "  return uv * 0.5 + 0.5;",
      "}",
      "float burstAt(float t, float cycle, float salt){",
      "  float idx = floor(t / cycle);",
      "  float local = mod(t, cycle);",
      "  float start = 0.4 + hash(vec2(idx, salt)) * (cycle - 1.6);",
      "  float dur = 0.07 + hash(vec2(idx, salt + 2.7)) * 0.23;",
      "  return step(start, local) * step(local, start + dur);",
      "}",
      "void main(){",
      "  vec2 uv = curve(v_uv);",
      "  uv = (uv - 0.5) * 0.94 + 0.5;",
      "  if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) { gl_FragColor = vec4(0.0, 0.0, 0.0, 1.0); return; }",
      "  uv.x += sin(uv.y * 38.0 + u_time * 1.7) * 0.0006;",
      "  float tick = floor(u_time * 1.7);",
      "  float tear = smoothstep(0.018, 0.0, abs(uv.y - hash(vec2(tick, 3.0))));",
      "  uv.x += tear * (hash(vec2(tick, 7.0)) - 0.5) * 0.006;",
      "  vec2 d = uv - 0.5;",
      "  float ca = 0.0012 + dot(d, d) * 0.007;",
      "  float r = texture2D(u_tex, uv + vec2(ca, 0.0)).r;",
      "  float g = texture2D(u_tex, uv).g;",
      "  float b = texture2D(u_tex, uv - vec2(ca, 0.0)).b;",
      "  vec3 col = vec3(r, g, b);",
      "  float scan = 0.86 + 0.14 * sin(gl_FragCoord.y * 2.0);",
      "  col *= scan;",
      "  float mx = mod(gl_FragCoord.x, 3.0);",
      "  vec3 mask = vec3(1.0);",
      "  if (mx < 1.0) mask.r = 1.09; else if (mx < 2.0) mask.g = 1.09; else mask.b = 1.09;",
      "  col *= mask;",
      "  float bandPos = fract(u_time * 0.07);",
      "  float band = smoothstep(0.055, 0.0, abs(uv.y - bandPos));",
      "  col += band * vec3(0.55, 0.7, 1.0) * 0.10;",
      "  float seed = floor(u_time * 18.0);",
      "  float n = hash(gl_FragCoord.xy * vec2(0.9, 1.35) + vec2(seed * 37.0, seed * 17.0));",
      "  float n2 = hash(gl_FragCoord.xy * 0.43 + seed * 91.0);",
      "  float streak = step(0.985, hash(vec2(floor(uv.y * 140.0), seed * 1.7))) * 0.35;",
      "  float warm = smoothstep(0.5, 0.78, u_time);",
      "  vec3 staticCol = vec3(n * (0.62 + 0.38 * n2) + streak) * scan * vec3(0.9, 0.96, 1.0);",
      "  float burst = max(burstAt(u_time, 7.3, 1.0), burstAt(u_time, 11.1, 5.0));",
      "  col = mix(col, staticCol, max(1.0 - warm, burst));",
      "  float lockFlash = exp(-pow((u_time - 0.52) * 28.0, 2.0));",
      "  col += lockFlash * 0.12;",
      "  col += (n - 0.5) * 0.05;",
      "  col *= 0.985 + 0.015 * sin(u_time * 41.0);",
      "  vec2 vv = uv * (1.0 - uv);",
      "  float vig = pow(clamp(vv.x * vv.y * 16.0, 0.0, 1.0), 0.2);",
      "  col *= mix(0.72, 1.0, vig);",
      "  float lum = max(col.r, max(col.g, col.b));",
      "  col += smoothstep(0.62, 1.0, lum) * 0.05;",
      "  gl_FragColor = vec4(col, 1.0);",
      "}"
    ].join("\n");

    function compile(type, src) {
      var sh = gl.createShader(type);
      gl.shaderSource(sh, src);
      gl.compileShader(sh);
      if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
        gl.deleteShader(sh);
        return null;
      }
      return sh;
    }

    var vs = compile(gl.VERTEX_SHADER, vertex);
    var fs = compile(gl.FRAGMENT_SHADER, fragment);
    if (!vs || !fs) return;

    var prog = gl.createProgram();
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return;
    gl.useProgram(prog);

    var buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    var loc = gl.getAttribLocation(prog, "a_pos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

    var uTex = gl.getUniformLocation(prog, "u_tex");
    var uTime = gl.getUniformLocation(prog, "u_time");
    var ready = false;
    var started = 0;

    function resize() {
      var dpr = Math.min(window.devicePixelRatio || 1, 1.5);
      var w = Math.max(2, Math.round(img.clientWidth * dpr));
      var h = Math.max(2, Math.round(w * (img.naturalHeight || 900) / (img.naturalWidth || 1180)));
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
        gl.viewport(0, 0, w, h);
      }
    }

    function draw(now) {
      gl.uniform1f(uTime, now);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    }

    function frame(now) {
      if (!ready) return;
      draw((now - started) / 1000);
      requestAnimationFrame(frame);
    }

    function start() {
      try {
        resize();
        var tex = gl.createTexture();
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, img);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        gl.uniform1i(uTex, 0);
        canvas.classList.add("on");
        img.classList.add("crt-off");
        ready = true;
        started = performance.now();
        window.addEventListener("resize", resize);
        if (prefersReduced) {
          draw(1.0);
        } else {
          requestAnimationFrame(frame);
        }
      } catch (e) {
        canvas.classList.remove("on");
        img.classList.remove("crt-off");
      }
    }

    if (img.complete && img.naturalWidth) {
      start();
    } else {
      img.addEventListener("load", start, { once: true });
    }
  }

  /* ---------- shader fog for final CTA ---------- */
  function initCtaFog() {
    var canvas = document.querySelector(".cta-fog");
    if (!canvas) return;
    var gl = canvas.getContext("webgl", { alpha: true, antialias: false, premultipliedAlpha: false });
    if (!gl) return;

    var vertex = [
      "attribute vec2 a_pos;",
      "void main(){ gl_Position = vec4(a_pos, 0.0, 1.0); }"
    ].join("\n");

    var fragment = [
      "precision highp float;",
      "uniform vec2 u_res;",
      "uniform float u_time;",
      "float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123); }",
      "float noise(vec2 p){",
      "  vec2 i = floor(p); vec2 f = fract(p);",
      "  f = f * f * (3.0 - 2.0 * f);",
      "  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x),",
      "             mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);",
      "}",
      "float fbm(vec2 p){",
      "  float v = 0.0; float a = 0.5;",
      "  for (int i = 0; i < 4; i++){ v += a * noise(p); p *= 2.02; a *= 0.5; }",
      "  return v;",
      "}",
      "void main(){",
      "  vec2 uv = gl_FragCoord.xy / u_res.xy;",
      "  vec2 p = (gl_FragCoord.xy - 0.5 * u_res.xy) / u_res.y;",
      "  float t = u_time * 0.035;",
      "  vec2 q = vec2(fbm(p * 1.15 + t), fbm(p * 1.15 + vec2(5.2, 1.3) - t));",
      "  vec2 r = vec2(fbm(p * 1.15 + 2.0 * q + vec2(1.7, 9.2) + 0.12 * t), fbm(p * 1.15 + 2.0 * q + vec2(8.3, 2.8) - 0.1 * t));",
      "  float f = fbm(p * 1.15 + 1.7 * r);",
      "  float fog = smoothstep(0.28, 0.68, f);",
      "  vec2 e = uv * (1.0 - uv);",
      "  float edgeFade = smoothstep(0.0, 0.16, e.x) * smoothstep(0.0, 0.16, e.y);",
      "  float radial = smoothstep(0.9, 0.3, length((uv - 0.5) * vec2(1.2, 1.0)));",
      "  float fade = edgeFade * radial;",
      "  vec3 col = mix(vec3(0.05, 0.13, 0.24), vec3(0.12, 0.30, 0.36), f * 0.55);",
      "  col += (hash(gl_FragCoord.xy + u_time) - 0.5) * 0.015;",
      "  gl_FragColor = vec4(col, fog * fade * 0.4);",
      "}"
    ].join("\n");

    function compile(type, src) {
      var sh = gl.createShader(type);
      gl.shaderSource(sh, src);
      gl.compileShader(sh);
      if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
        gl.deleteShader(sh);
        return null;
      }
      return sh;
    }

    var vs = compile(gl.VERTEX_SHADER, vertex);
    var fs = compile(gl.FRAGMENT_SHADER, fragment);
    if (!vs || !fs) return;

    var prog = gl.createProgram();
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return;
    gl.useProgram(prog);

    var buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    var loc = gl.getAttribLocation(prog, "a_pos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
    var uRes = gl.getUniformLocation(prog, "u_res");
    var uTime = gl.getUniformLocation(prog, "u_time");

    function resize() {
      var dpr = Math.min(window.devicePixelRatio || 1, 1.5);
      var w = Math.max(2, Math.round(canvas.clientWidth * dpr));
      var h = Math.max(2, Math.round(canvas.clientHeight * dpr));
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
        gl.viewport(0, 0, w, h);
      }
    }
    resize();
    window.addEventListener("resize", resize);
    gl.clearColor(0, 0, 0, 0);

    function draw(t) {
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.uniform2f(uRes, canvas.width, canvas.height);
      gl.uniform1f(uTime, t);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    }

    var visible = false;
    if ("IntersectionObserver" in window) {
      new IntersectionObserver(function (entries) {
        visible = entries[0].isIntersecting;
      }, { threshold: 0 }).observe(canvas);
    } else {
      visible = true;
    }

    if (prefersReduced) {
      draw(0);
      return;
    }
    var start = performance.now();
    function frame(now) {
      if (visible && !document.hidden) draw((now - start) / 1000);
      requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);
  }

  /* ---------- interlude camera ---------- */
  function initInterludeCamera() {
    var heroCam = document.querySelector(".shape-cam .cam3d");
    var mount = document.querySelector(".interlude-cam");
    if (!heroCam || !mount) return;

    var cam = heroCam.cloneNode(true);
    mount.appendChild(cam);

    var pointer = { x: window.innerWidth / 2, y: window.innerHeight / 2, active: false };
    window.addEventListener("pointermove", function (e) {
      pointer.x = e.clientX;
      pointer.y = e.clientY;
      pointer.active = true;
    }, { passive: true });
    document.documentElement.addEventListener("pointerleave", function () {
      pointer.active = false;
    });
    window.addEventListener("blur", function () {
      pointer.active = false;
    });

    var visible = false;
    if ("IntersectionObserver" in window) {
      new IntersectionObserver(function (entries) {
        visible = entries[0].isIntersecting;
      }, { threshold: 0 }).observe(mount);
    } else {
      visible = true;
    }

    if (prefersReduced) {
      cam.style.transform = "rotateY(-8deg) rotateX(4deg)";
      return;
    }

    var yaw = 0;
    var pitch = 0;
    var start = performance.now();
    var flourish = {
      start: -Infinity,
      duration: 1080,
      direction: 1,
      radiusX: 26,
      radiusY: 11,
      tilt: 5,
      pitch: 6,
      lift: 0.08
    };
    mount.addEventListener("camera-flourish", function () {
      var direction = Math.random() < 0.5 ? -1 : 1;
      flourish = {
        start: performance.now(),
        duration: 920 + Math.random() * 300,
        direction: direction,
        radiusX: 19 + Math.random() * 16,
        radiusY: 8 + Math.random() * 11,
        tilt: 3.5 + Math.random() * 5.5,
        pitch: 3 + Math.random() * 7,
        lift: 0.055 + Math.random() * 0.07
      };
    });

    function frame(now) {
      if (visible && !document.hidden) {
        var r = mount.getBoundingClientRect();
        var cx = r.left + r.width / 2;
        var cy = r.top + r.height / 2;
        var dx = pointer.x - cx;
        var dy = pointer.y - cy;
        if (!pointer.active || Math.hypot(dx, dy) > 650) {
          dx = window.innerWidth / 2 - cx;
          dy = window.innerHeight / 2 - cy;
        }
        var rangeX = Math.max(360, window.innerWidth * 0.45);
        var rangeY = Math.max(260, window.innerHeight * 0.45);
        var targetYaw = Math.max(-29, Math.min(29, dx / rangeX * 29));
        var targetPitch = Math.max(-16, Math.min(16, -dy / rangeY * 16));
        var t = (now - start) / 1000;
        var ease = 0.045 + 0.025 * (0.5 + 0.5 * Math.sin(t * 1.4));
        yaw += (targetYaw - yaw) * ease;
        pitch += (targetPitch - pitch) * ease;
        var swayYaw = Math.sin(t * 0.7) * 1.8;
        var swayPitch = Math.sin(t * 0.9 + 1.1) * 1.1;
        var bob = Math.sin(t * 0.8) * 4;
        var elapsed = now - flourish.start;
        var celebrating = elapsed >= 0 && elapsed < flourish.duration;
        var flourishX = 0;
        var flourishY = 0;
        var flourishYaw = 0;
        var flourishTilt = 0;
        var flourishPitch = 0;
        var flourishScale = 1;
        if (celebrating) {
          var progress = elapsed / flourish.duration;
          var orbit = progress * Math.PI * 2 * flourish.direction;
          var spin = progress < 0.5
            ? 4 * Math.pow(progress, 3)
            : 1 - Math.pow(-2 * progress + 2, 3) / 2;
          flourishX = Math.sin(orbit) * flourish.radiusX;
          flourishY = (Math.cos(orbit) - 1) * flourish.radiusY;
          flourishYaw = spin * 360 * flourish.direction;
          flourishTilt = Math.sin(orbit) * flourish.tilt;
          flourishPitch = Math.sin(orbit * 2) * flourish.pitch;
          flourishScale = 1 + Math.sin(progress * Math.PI) * flourish.lift;
          mount.classList.add("is-celebrating");
        } else {
          mount.classList.remove("is-celebrating");
        }
        cam.style.transform = "translate3d(" + flourishX.toFixed(2) + "px," + (bob + flourishY).toFixed(2) + "px,0) rotateY(" + (yaw + swayYaw + flourishYaw).toFixed(2) + "deg) rotateX(" + (pitch + swayPitch + flourishPitch).toFixed(2) + "deg) rotateZ(" + flourishTilt.toFixed(2) + "deg) scale(" + flourishScale.toFixed(3) + ")";
      }
      requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);
  }

  function initCameraShutters() {
    var flash = document.querySelector(".shutter-flash");
    var audioContext = null;
    var readyAt = -Infinity;
    var cooldownMs = 420;
    var celebrationLockMs = 1500;
    var comboWindowMs = 1400;
    var comboCount = 3;
    var shotTimes = [];

    function runAudio(start) {
      var AudioCtor = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtor) return;
      if (!audioContext) audioContext = new AudioCtor();
      if (audioContext.state === "suspended") {
        audioContext.resume().then(start).catch(function () {});
      } else {
        start();
      }
    }

    function playShutter() {
      function playBursts() {
        var now = audioContext.currentTime + 0.006;

        function burst(start, duration, level, cutoff) {
          var length = Math.max(1, Math.ceil(audioContext.sampleRate * duration));
          var buffer = audioContext.createBuffer(1, length, audioContext.sampleRate);
          var samples = buffer.getChannelData(0);
          for (var i = 0; i < length; i += 1) {
            var envelope = Math.pow(1 - i / length, 3);
            samples[i] = (Math.random() * 2 - 1) * envelope;
          }
          var source = audioContext.createBufferSource();
          var filter = audioContext.createBiquadFilter();
          var gain = audioContext.createGain();
          source.buffer = buffer;
          filter.type = "highpass";
          filter.frequency.setValueAtTime(cutoff, start);
          gain.gain.setValueAtTime(level, start);
          gain.gain.exponentialRampToValueAtTime(0.0001, start + duration);
          source.connect(filter);
          filter.connect(gain);
          gain.connect(audioContext.destination);
          source.start(start);
          source.stop(start + duration);
        }

        function bodyClick(start, fromHz, toHz, duration, level) {
          var oscillator = audioContext.createOscillator();
          var gain = audioContext.createGain();
          oscillator.type = "triangle";
          oscillator.frequency.setValueAtTime(fromHz, start);
          oscillator.frequency.exponentialRampToValueAtTime(toHz, start + duration);
          gain.gain.setValueAtTime(level, start);
          gain.gain.exponentialRampToValueAtTime(0.0001, start + duration);
          oscillator.connect(gain);
          gain.connect(audioContext.destination);
          oscillator.start(start);
          oscillator.stop(start + duration);
        }

        burst(now, 0.034, 0.38, 1180);
        bodyClick(now, 190, 72, 0.055, 0.13);
        burst(now + 0.078, 0.026, 0.23, 2050);
        bodyClick(now + 0.078, 150, 88, 0.04, 0.075);
      }

      runAudio(playBursts);
    }

    function playFlourish() {
      function playWhoosh() {
        var start = audioContext.currentTime + 0.22;
        var duration = 0.72;
        var length = Math.ceil(audioContext.sampleRate * duration);
        var buffer = audioContext.createBuffer(1, length, audioContext.sampleRate);
        var samples = buffer.getChannelData(0);
        for (var i = 0; i < length; i += 1) {
          var envelope = Math.sin(Math.PI * i / length);
          samples[i] = (Math.random() * 2 - 1) * envelope * envelope;
        }

        var source = audioContext.createBufferSource();
        var filter = audioContext.createBiquadFilter();
        var gain = audioContext.createGain();
        source.buffer = buffer;
        filter.type = "bandpass";
        filter.Q.setValueAtTime(0.9, start);
        filter.frequency.setValueAtTime(520, start);
        filter.frequency.exponentialRampToValueAtTime(2700, start + 0.3);
        filter.frequency.exponentialRampToValueAtTime(860, start + duration);
        gain.gain.setValueAtTime(0.0001, start);
        gain.gain.linearRampToValueAtTime(0.12, start + 0.22);
        gain.gain.exponentialRampToValueAtTime(0.0001, start + duration);
        source.connect(filter);
        filter.connect(gain);

        var panner = null;
        if (audioContext.createStereoPanner) {
          panner = audioContext.createStereoPanner();
          panner.pan.setValueAtTime(-0.34, start);
          panner.pan.linearRampToValueAtTime(0.38, start + 0.34);
          panner.pan.linearRampToValueAtTime(0, start + duration);
          gain.connect(panner);
          panner.connect(audioContext.destination);
        } else {
          gain.connect(audioContext.destination);
        }
        source.start(start);
        source.stop(start + duration);

        var glide = audioContext.createOscillator();
        var glideGain = audioContext.createGain();
        glide.type = "sine";
        glide.frequency.setValueAtTime(210, start);
        glide.frequency.exponentialRampToValueAtTime(920, start + 0.3);
        glide.frequency.exponentialRampToValueAtTime(410, start + duration);
        glideGain.gain.setValueAtTime(0.0001, start);
        glideGain.gain.linearRampToValueAtTime(0.045, start + 0.24);
        glideGain.gain.exponentialRampToValueAtTime(0.0001, start + duration);
        glide.connect(glideGain);
        glideGain.connect(audioContext.destination);
        glide.start(start);
        glide.stop(start + duration);
      }

      runAudio(playWhoosh);
    }

    function fire(trigger) {
      var now = performance.now();
      if (now < readyAt) return;
      readyAt = now + cooldownMs;
      playShutter();

      trigger.classList.remove("is-firing");
      void trigger.offsetWidth;
      trigger.classList.add("is-firing");
      trigger.setAttribute("aria-disabled", "true");

      if (flash) {
        flash.classList.remove("is-firing");
        void flash.offsetWidth;
        flash.classList.add("is-firing");
      }

      shotTimes = shotTimes.filter(function (time) {
        return now - time <= comboWindowMs;
      });
      shotTimes.push(now);
      if (shotTimes.length >= comboCount) {
        shotTimes.length = 0;
        readyAt = now + celebrationLockMs;
        playFlourish();
        window.setTimeout(function () {
          trigger.dispatchEvent(new Event("camera-flourish"));
        }, 220);
      }

      window.setTimeout(function () {
        trigger.classList.remove("is-firing");
      }, cooldownMs);

      window.setTimeout(function () {
        trigger.removeAttribute("aria-disabled");
      }, Math.max(cooldownMs, readyAt - now));
    }

    document.querySelectorAll("[data-camera-shutter]").forEach(function (trigger) {
      trigger.addEventListener("click", function () {
        fire(trigger);
      });
      trigger.addEventListener("keydown", function (event) {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        fire(trigger);
      });
    });
  }

  function initFooterPeekCamera() {
    var heroCam = document.querySelector(".shape-cam .cam3d");
    var downloadButton = document.querySelector(".final-cta .btn-primary");
    var mount = document.querySelector("[data-footer-peek]");
    var trigger = mount && mount.querySelector(".footer-peek-trigger");
    if (!heroCam || !downloadButton || !mount || !trigger) return;

    var cam = heroCam.cloneNode(true);
    trigger.appendChild(cam);

    var dwellMs = 30000;
    var hideMinMs = 30000;
    var hideMaxMs = 50000;
    var bottomSince = 0;
    var hiddenUntil = 0;
    var visible = false;
    var retreating = false;
    var side = "left";
    var activeStart = 0;
    var yaw = 0;
    var pitch = 0;
    var pointer = { x: window.innerWidth / 2, y: window.innerHeight / 2, active: false };

    function isInPeekZone() {
      var rect = downloadButton.getBoundingClientRect();
      var visibleHeight = Math.min(rect.bottom, window.innerHeight) - Math.max(rect.top, 0);
      return visibleHeight >= rect.height * 0.55 && rect.bottom > 70;
    }

    function show() {
      if (visible || retreating || performance.now() < hiddenUntil) return;
      side = Math.random() < 0.5 ? "left" : "right";
      visible = true;
      activeStart = performance.now();
      yaw = side === "left" ? 20 : -20;
      pitch = 0;
      mount.classList.remove("is-left", "is-right", "is-retreating");
      void mount.offsetWidth;
      mount.classList.add("is-peeking", "is-" + side);
      mount.setAttribute("aria-hidden", "false");
      trigger.tabIndex = 0;
      if (prefersReduced) cam.style.transform = "rotateY(" + yaw + "deg) rotateX(-3deg)";
    }

    function hide(cooldown) {
      if (!visible || retreating) return;
      retreating = true;
      if (cooldown > 0) hiddenUntil = performance.now() + cooldown;
      mount.setAttribute("aria-hidden", "true");
      trigger.tabIndex = -1;
      mount.classList.add("is-retreating");
      window.setTimeout(function () {
        visible = false;
        retreating = false;
        mount.classList.remove("is-peeking", "is-left", "is-right", "is-retreating");
      }, 300);
    }

    function escape() {
      hide(hideMinMs + Math.random() * (hideMaxMs - hideMinMs));
    }

    function updatePresence() {
      if (!isInPeekZone()) {
        bottomSince = 0;
        if (visible) hide(0);
      } else if (!bottomSince) {
        bottomSince = performance.now();
      }
    }

    trigger.addEventListener("click", function () {
      escape();
    });
    trigger.addEventListener("pointerenter", function () {
      escape();
    });

    window.addEventListener("scroll", updatePresence, { passive: true });
    window.addEventListener("resize", updatePresence);
    document.addEventListener("visibilitychange", function () {
      if (document.hidden) {
        if (visible) hide(0);
      } else {
        updatePresence();
      }
    });

    window.addEventListener("pointermove", function (event) {
      pointer.x = event.clientX;
      pointer.y = event.clientY;
      pointer.active = true;
      if (visible && !retreating) {
        var rect = trigger.getBoundingClientRect();
        var cx = rect.left + rect.width / 2;
        var cy = rect.top + rect.height / 2;
        var distance = Math.hypot(event.clientX - cx, event.clientY - cy);
        if (distance < 170) escape();
      }
    }, { passive: true });

    window.setInterval(function () {
      if (document.hidden) return;
      updatePresence();
      var now = performance.now();
      if (!visible && !retreating && now >= hiddenUntil && bottomSince && now - bottomSince >= dwellMs) show();
    }, 500);

    if (!prefersReduced) {
      function frame(now) {
        if (visible && !document.hidden) {
          var rect = trigger.getBoundingClientRect();
          var cx = rect.left + rect.width / 2;
          var cy = rect.top + rect.height / 2;
          var targetYaw = side === "left" ? 22 : -22;
          var targetPitch = -3;
          if (pointer.active) {
            targetYaw = Math.max(-30, Math.min(30, (pointer.x - cx) / 220 * 28));
            targetPitch = Math.max(-11, Math.min(9, -(pointer.y - cy) / 190 * 10));
          }
          yaw += (targetYaw - yaw) * 0.2;
          pitch += (targetPitch - pitch) * 0.18;
          var t = (now - activeStart) / 1000;
          var scanYaw = Math.sin(t * 0.72) * 3.6 + Math.sin(t * 1.37) * 1.4;
          var scanPitch = Math.sin(t * 0.93 + 0.7) * 1.5;
          var bob = Math.sin(t * 0.88) * 3 + Math.sin(t * 1.71) * 1.2;
          cam.style.transform = "rotateY(" + (yaw + scanYaw).toFixed(2) + "deg) rotateX(" + (pitch + scanPitch).toFixed(2) + "deg) translateY(" + bob.toFixed(2) + "px)";
        }
        requestAnimationFrame(frame);
      }
      requestAnimationFrame(frame);
    }

    updatePresence();
  }

  /* ---------- nav ---------- */
  function initNav() {
    var nav = document.getElementById("nav");
    if (!nav) return;
    var onScroll = function () {
      nav.classList.toggle("scrolled", window.scrollY > 24);
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
  }

  /* ---------- reveal ---------- */
  function initReveal() {
    var items = document.querySelectorAll(".reveal");
    if (!("IntersectionObserver" in window)) {
      items.forEach(function (el) { el.classList.add("in"); });
      return;
    }
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          var el = entry.target;
          var delay = parseInt(el.getAttribute("data-delay") || "0", 10);
          el.style.setProperty("--delay", delay + "ms");
          el.classList.add("in");
          if (el.hasAttribute("data-scramble")) {
            scrambleTo(el, t(el.getAttribute("data-i18n")));
          }
          io.unobserve(el);
        }
      });
    }, { threshold: 0.12, rootMargin: "0px 0px -40px 0px" });
    items.forEach(function (el) { io.observe(el); });
  }

  /* ---------- counters ---------- */
  function initCounters() {
    var counters = document.querySelectorAll("[data-count]");
    if (!counters.length) return;
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        var el = entry.target;
        var target = parseInt(el.getAttribute("data-count"), 10);
        io.unobserve(el);
        if (prefersReduced) {
          el.textContent = target;
          return;
        }
        var start = performance.now();
        var dur = 1400;
        var step = function (now) {
          var tt = Math.min((now - start) / dur, 1);
          var eased = 1 - Math.pow(1 - tt, 3);
          el.textContent = Math.round(target * eased);
          if (tt < 1) requestAnimationFrame(step);
        };
        requestAnimationFrame(step);
      });
    }, { threshold: 0.6 });
    counters.forEach(function (el) { io.observe(el); });
  }

  /* ---------- REC timer ---------- */
  function initRecTimer() {
    var el = document.getElementById("recTime");
    if (!el || prefersReduced) return;
    var seconds = 0;
    setInterval(function () {
      seconds++;
      var m = Math.floor(seconds / 60);
      var s = seconds % 60;
      el.textContent = (m < 10 ? "0" + m : m) + ":" + (s < 10 ? "0" + s : s);
    }, 1000);
  }

  /* ---------- conversion progress ---------- */
  function initConversionProgress() {
    var card = document.querySelector(".convert-card");
    var fill = document.querySelector(".convert-progress i");
    var percent = document.getElementById("convertPercent");
    if (!card || !fill || !percent) return;

    var target = 85;
    var duration = 5200;
    var started = false;
    var lastPct = -1;
    var percentDigits = percent.querySelectorAll(".wobble-letter");

    function setValue(value) {
      fill.style.setProperty("--p", value.toFixed(2) + "%");
      var pct = Math.round(value);
      if (pct !== lastPct) {
        var text = String(pct);
        if (percentDigits[0]) percentDigits[0].textContent = text.length > 1 ? text[0] : "";
        if (percentDigits[1]) percentDigits[1].textContent = text.length > 1 ? text[1] : text[0];
        lastPct = pct;
      }
    }

    function run() {
      if (started) return;
      started = true;
      if (prefersReduced) {
        setValue(target);
        return;
      }
      var start = performance.now();
      function frame(now) {
        var t = Math.min((now - start) / duration, 1);
        var eased = 1 - Math.pow(1 - t, 3);
        setValue(target * eased);
        if (t < 1) {
          requestAnimationFrame(frame);
        } else {
          setValue(target);
        }
      }
      requestAnimationFrame(frame);
    }

    if ("IntersectionObserver" in window) {
      var io = new IntersectionObserver(function (entries) {
        if (entries[0].isIntersecting) {
          run();
          io.disconnect();
        }
      }, { threshold: 0.35 });
      io.observe(card);
    } else {
      run();
    }
  }

  /* ---------- 3D tilt ---------- */
  function initTilt() {
    if (prefersReduced || !window.matchMedia("(hover: hover)").matches) return;
    var items = document.querySelectorAll("[data-tilt]");
    items.forEach(function (el) {
      var media = el.querySelector(".pillar-shot, .bento-media");
      el.addEventListener("pointermove", function (e) {
        var r = el.getBoundingClientRect();
        var px = (e.clientX - r.left) / r.width - 0.5;
        var py = (e.clientY - r.top) / r.height - 0.5;
        var rx = -py * 2.4;
        var ry = px * 2.4;
        el.style.transform = "perspective(900px) rotateX(" + rx.toFixed(2) + "deg) rotateY(" + ry.toFixed(2) + "deg) translateY(-2px)";
        if (media) {
          media.style.setProperty("--tilt-angle", (rx * 3).toFixed(2) + "deg");
          media.style.setProperty("--mask-shift", (ry * 4).toFixed(2) + "%");
          media.style.setProperty("--light-o", "1");
        }
      });
      el.addEventListener("pointerleave", function () {
        el.style.transform = "";
        if (media) media.style.setProperty("--light-o", "0");
      });
    });
  }

  /* ---------- magnetic buttons ---------- */
  function initMagnetic() {
    if (prefersReduced || !window.matchMedia("(hover: hover)").matches) return;
    document.querySelectorAll(".magnetic").forEach(function (btn) {
      btn.addEventListener("pointermove", function (e) {
        var r = btn.getBoundingClientRect();
        var x = e.clientX - r.left - r.width / 2;
        var y = e.clientY - r.top - r.height / 2;
        btn.style.transform = "translate(" + x * 0.18 + "px," + y * 0.24 + "px)";
      });
      btn.addEventListener("pointerleave", function () {
        btn.style.transform = "";
      });
    });
  }

  /* ---------- language switcher ---------- */
  function initLanguageSwitcher() {
    document.querySelectorAll(".lang-btn").forEach(function (btn) {
      btn.addEventListener("click", function () {
        setLang(btn.getAttribute("data-lang"));
      });
    });
  }

  /* ---------- palette shift on scroll ---------- */
  function hexToRgb(hex) {
    hex = hex.replace("#", "");
    if (hex.length === 3) hex = hex.split("").map(function (c) { return c + c; }).join("");
    var n = parseInt(hex, 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }

  function lerpColor(a, b, t) {
    var A = hexToRgb(a);
    var B = hexToRgb(b);
    return "rgb(" + Math.round(A[0] + (B[0] - A[0]) * t) + "," + Math.round(A[1] + (B[1] - A[1]) * t) + "," + Math.round(A[2] + (B[2] - A[2]) * t) + ")";
  }

  function initPaletteShift() {
    var root = document.documentElement;
    var pairs = {
      "--g1": ["#1d4ed8", "#4c1d95"],
      "--g2": ["#3b82f6", "#7c3aed"],
      "--g3": ["#22d3ee", "#a855f7"],
      "--g4": ["#7dd3fc", "#c4b5fd"],
      "--cyan": ["#22d3ee", "#a855f7"]
    };
    function update() {
      var max = document.documentElement.scrollHeight - window.innerHeight;
      var raw = max > 0 ? Math.min(Math.max(window.scrollY / max, 0), 1) : 0;
      var p = raw * 0.3;
      Object.keys(pairs).forEach(function (key) {
        root.style.setProperty(key, lerpColor(pairs[key][0], pairs[key][1], p));
      });
      shaderMix = p;
    }
    window.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    update();
  }

  /* ---------- run ---------- */
  setLang(detectLang());
  initShader();
  initCrt();
  initCtaFog();
  initInterludeCamera();
  initCameraShutters();
  initFooterPeekCamera();
  initNav();
  initReveal();
  initCounters();
  initRecTimer();
  initConversionProgress();
  initTilt();
  initMagnetic();
  initLanguageSwitcher();
  initPaletteShift();
})();
