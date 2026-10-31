/** @type {import('tailwindcss').Config} */
// Palette follows Palantir's Blueprint dark theme (Foundry / Gotham look).
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        pl: {
          bg0: "#111418", bg1: "#1C2127", bg2: "#252A31", bg3: "#2F343C", bg4: "#383E47",
          line: "#2F343C", line2: "#404854",
          text: "#F6F7F9", mute: "#ABB3BF", dim: "#738091",
          blue: "#4C90F0", blue2: "#2D72D2", green: "#32A467", orange: "#EC9A3C",
          red: "#E76A6E", gold: "#FBD065", cyan: "#68C1EE", violet: "#BDADFF", rose: "#FF66A1",
        },
      },
      fontFamily: {
        sans: ["Inter", "-apple-system", "Segoe UI", "system-ui", "sans-serif"],
        mono: ["JetBrains Mono", "SFMono-Regular", "Consolas", "ui-monospace", "monospace"],
      },
      borderRadius: { DEFAULT: "2px", sm: "2px", md: "3px" },
      fontSize: { "2xs": ["10px", "14px"] },
      keyframes: { blink: { "0%,100%": { opacity: "1" }, "50%": { opacity: ".35" } },
                   sweep: { "0%": { transform: "translateX(-100%)" }, "100%": { transform: "translateX(300%)" } } },
      animation: { blink: "blink 1.4s ease-in-out infinite", sweep: "sweep 1.6s linear infinite" },
    },
  },
  plugins: [],
};
