// Baleia Timmy – mascote em desenho animado (design provisório até a arte original ser adotada)
// Estados: idle · wave · swim · happy · sad

export function timmySVG() {
  // Primeiro design (aprovado) do Timmy; partes animáveis individualmente
  return `
<svg viewBox="-30 -40 260 220" xmlns="http://www.w3.org/2000/svg" aria-label="Wal Timmy" role="img">
  <ellipse class="t-shadow" cx="100" cy="166" rx="70" ry="7" fill="#0b2545" opacity=".12"/>
  <g class="t-waves">
    <path class="t-wave1" d="M-70 150 q20 -12 40 0 t40 0 t40 0 t40 0 t40 0 t40 0 t40 0 t40 0 t40 0 V190 H-70Z" fill="#38bdf8" opacity=".45"/>
    <path class="t-wave2" d="M-90 158 q20 -10 40 0 t40 0 t40 0 t40 0 t40 0 t40 0 t40 0 t40 0 t40 0 V190 H-90Z" fill="#1e88e5" opacity=".5"/>
  </g>
  <g class="t-whole">
    <g class="t-spout" fill="#38bdf8">
      <path d="M100 6c4 10 2 18-4 24 8-4 16-3 22 2-7-1-13 1-17 6l-2-3c-4-5-10-7-17-6 6-5 14-6 22-2-6-6-8-14-4-21z" opacity=".85"/>
      <circle cx="84" cy="2" r="3" opacity=".7"/><circle cx="118" cy="-2" r="2.5" opacity=".7"/>
    </g>
    <g class="t-tail"></g>
    <g class="t-body">
      <path d="M30 92c0-30 30-50 66-50 40 0 70 22 70 52 0 12-4 20-10 26 10 0 20-6 26-16 4 14-4 30-22 34-8 2-16 0-22-4-12 6-28 8-44 8-38 0-64-20-64-50z" fill="#1e88e5"/>
      <path d="M40 104c10 22 34 32 60 32 22 0 40-6 50-16-6 18-28 28-54 28-30 0-52-16-56-44z" fill="#bfe3ff"/>
      <g class="t-eye">
        <circle cx="74" cy="84" r="9" fill="#fff"/>
        <circle class="t-pupil" cx="76" cy="85" r="5" fill="#0b2545"/>
        <circle cx="78" cy="83" r="1.6" fill="#fff"/>
      </g>
      <path class="t-brow" d="M66 70 q8 -5 17 0" stroke="#0b2545" stroke-width="3" fill="none" stroke-linecap="round"/>
      <path class="t-mouth t-smile" d="M60 104c10 8 24 8 34 0" stroke="#0b2545" stroke-width="3.5" fill="none" stroke-linecap="round"/>
      <path class="t-mouth t-open" d="M60 103 q17 18 34 0 q-17 5 -34 0 Z" fill="#7a1f2b" stroke="#0b2545" stroke-width="3" stroke-linejoin="round"/>
      <path class="t-mouth t-sad" d="M62 111 q15 -9 30 0" stroke="#0b2545" stroke-width="3.5" fill="none" stroke-linecap="round"/>
      <path class="t-tear" d="M68 94 q-5 8 0 11 q5 -3 0 -11 Z" fill="#7dd3fc"/>
      <circle cx="56" cy="98" r="5" fill="#ff8fa3" opacity=".7"/>
      <g class="t-flip"><path d="M98 112c8-2 16 4 18 12-8 2-16-2-18-12z" fill="#1565c0"/></g>
    </g>
  </g>
  <g class="t-bubbles" fill="none" stroke="#7dd3fc" stroke-width="2.5">
    <circle class="b1" cx="22" cy="104" r="6"/><circle class="b2" cx="10" cy="120" r="4"/><circle class="b3" cx="30" cy="124" r="3"/>
  </g>
  <g class="t-sparkles" fill="#ffd54f" stroke="#0b2545" stroke-width="1.6" stroke-linejoin="round">
    <path class="s1" d="M8 20 l5 11 l11 5 l-11 5 l-5 11 l-5 -11 l-11 -5 l11 -5 Z"/>
    <path class="s2" d="M176 6 l4 8 l8 4 l-8 4 l-4 8 l-4 -8 l-8 -4 l8 -4 Z"/>
    <path class="s3" d="M206 70 l3 6 l6 3 l-6 3 l-3 6 l-3 -6 l-6 -3 l6 -3 Z"/>
  </g>
</svg>`;
}

/** Insere o Timmy em um elemento. */
export function mountTimmy(el, state = "idle") {
  el.classList.add("timmy");
  el.innerHTML = timmySVG();
  setTimmy(el, state);
}

/** Troca de estado: idle · wave · swim · happy · sad. `then` = estado seguinte após ms. */
export function setTimmy(el, state, then, ms = 2400) {
  if (!el) return;
  el.classList.remove("idle", "wave", "swim", "happy", "sad");
  // Reinicia a animação
  void el.offsetWidth;
  el.classList.add(state);
  clearTimeout(el._t);
  if (then) el._t = setTimeout(() => setTimmy(el, then), ms);
}
