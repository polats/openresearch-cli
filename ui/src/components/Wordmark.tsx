// Crucible wordmark: a molten-vessel mark + name, sized by the parent's font-size.
// The mark recolors with the active theme (fill = --primary). Single source for
// the brand lockup (home, chat empty state, onboarding).
export function Wordmark() {
  return (
    <span className="wordmark">
      <svg viewBox="0 0 100 100" aria-hidden="true">
        {/* brand tile */}
        <rect width="100" height="100" rx="24" fill="var(--primary)" />
        {/* molten drop falling in */}
        <path
          d="M50 19c4.6 6.4 7.2 10.4 7.2 13.7a7.2 7.2 0 1 1-14.4 0C42.8 29.4 45.4 25.4 50 19z"
          fill="#fff"
        />
        {/* crucible: a cup wider at the rim, curving to a rounded base */}
        <path
          d="M27 44h46l-4.8 21.4A17 10 0 0 1 50 75a17 10 0 0 1-18.2-9.6L27 44z"
          fill="#fff"
        />
        {/* hollow opening */}
        <ellipse cx="50" cy="44" rx="23" ry="5.4" fill="var(--primary)" />
        <ellipse cx="50" cy="44" rx="23" ry="5.4" fill="none" stroke="#fff" strokeWidth="3.4" />
      </svg>
      Crucible
    </span>
  );
}
