// vitest.config.ts
import { defineConfig } from "file:///D:/project/super-agent/office-agent-tauri/node_modules/vitest/dist/config.js";
import react from "file:///D:/project/super-agent/office-agent-tauri/node_modules/@vitejs/plugin-react/dist/index.js";
import path from "node:path";

// package.json
var package_default = {
  name: "agent-office",
  version: "0.1.0",
  private: true,
  description: "Multi-agent office harness \u2014 Tauri (Rust) port of the Electron app",
  type: "module",
  scripts: {
    dev: "vite",
    build: "tsc --noEmit && vite build",
    test: "vitest run && cargo test --manifest-path src-tauri/Cargo.toml",
    "test:ui": "vitest run",
    "test:rs": "cargo test --manifest-path src-tauri/Cargo.toml",
    tauri: "tauri",
    "tauri:dev": "tauri dev",
    "tauri:build": "tauri build"
  },
  dependencies: {
    "@codemirror/lang-css": "^6.3.1",
    "@codemirror/lang-html": "^6.4.12",
    "@codemirror/lang-javascript": "^6.2.5",
    "@codemirror/lang-json": "^6.0.2",
    "@codemirror/lang-markdown": "^6.5.2",
    "@codemirror/lang-python": "^6.2.1",
    "@codemirror/lang-yaml": "^6.1.3",
    "@codemirror/language": "^6.12.4",
    "@codemirror/state": "^6.7.6",
    "@codemirror/view": "^6.43.13",
    "@lezer/highlight": "^1.2.5",
    "@monaco-editor/react": "^4.7.0",
    "@openai/agents-realtime": "^0.11.8",
    "@tauri-apps/api": "^2.9.0",
    "@uiw/react-codemirror": "^4.25.12",
    "@xterm/addon-fit": "^0.10.0",
    "@xterm/addon-unicode11": "^0.9.0",
    "@xterm/addon-webgl": "^0.19.0",
    "@xterm/xterm": "^5.5.0",
    i18next: "^26.4.2",
    "i18next-browser-languagedetector": "^8.2.1",
    "monaco-editor": "^0.52.2",
    "pixi.js": "^8.22.0",
    react: "^18.3.1",
    "react-dom": "^18.3.1",
    "react-i18next": "^17.0.15",
    "react-markdown": "^10.1.0",
    "remark-gfm": "^4.0.1",
    zustand: "^4.5.5"
  },
  devDependencies: {
    "@tauri-apps/cli": "^2.9.0",
    "@testing-library/dom": "^10.4.2",
    "@testing-library/react": "^16.3.3",
    "@testing-library/user-event": "^14.6.7",
    "@types/react": "^18.3.0",
    "@types/react-dom": "^18.3.0",
    "@vitejs/plugin-react": "^4.3.2",
    jsdom: "^29.1.1",
    typescript: "^5.6.3",
    vite: "^5.4.8",
    vitest: "^3.2.7"
  }
};

// vitest.config.ts
var __vite_injected_original_dirname = "D:\\project\\super-agent\\office-agent-tauri";
var vitest_config_default = defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(package_default.version)
  },
  resolve: {
    alias: {
      "@": path.resolve(__vite_injected_original_dirname, "src"),
      "@shared": path.resolve(__vite_injected_original_dirname, "src/shared"),
      "@brand": path.resolve(__vite_injected_original_dirname, "docs")
    }
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["src/test/setup.ts"],
    // Pixi/xterm/Monaco leave timers + RAF loops behind; they are mocked per
    // test file anyway, and this keeps the suite from hanging the run.
    teardownTimeout: 5e3
  },
  assetsInclude: ["**/*.tmj"]
});
export {
  vitest_config_default as default
};
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsidml0ZXN0LmNvbmZpZy50cyIsICJwYWNrYWdlLmpzb24iXSwKICAic291cmNlc0NvbnRlbnQiOiBbImNvbnN0IF9fdml0ZV9pbmplY3RlZF9vcmlnaW5hbF9kaXJuYW1lID0gXCJEOlxcXFxwcm9qZWN0XFxcXHN1cGVyLWFnZW50XFxcXG9mZmljZS1hZ2VudC10YXVyaVwiO2NvbnN0IF9fdml0ZV9pbmplY3RlZF9vcmlnaW5hbF9maWxlbmFtZSA9IFwiRDpcXFxccHJvamVjdFxcXFxzdXBlci1hZ2VudFxcXFxvZmZpY2UtYWdlbnQtdGF1cmlcXFxcdml0ZXN0LmNvbmZpZy50c1wiO2NvbnN0IF9fdml0ZV9pbmplY3RlZF9vcmlnaW5hbF9pbXBvcnRfbWV0YV91cmwgPSBcImZpbGU6Ly8vRDovcHJvamVjdC9zdXBlci1hZ2VudC9vZmZpY2UtYWdlbnQtdGF1cmkvdml0ZXN0LmNvbmZpZy50c1wiO2ltcG9ydCB7IGRlZmluZUNvbmZpZyB9IGZyb20gJ3ZpdGVzdC9jb25maWcnO1xuaW1wb3J0IHJlYWN0IGZyb20gJ0B2aXRlanMvcGx1Z2luLXJlYWN0JztcbmltcG9ydCBwYXRoIGZyb20gJ25vZGU6cGF0aCc7XG5pbXBvcnQgcGtnIGZyb20gJy4vcGFja2FnZS5qc29uJyB3aXRoIHsgdHlwZTogJ2pzb24nIH07XG5cbmV4cG9ydCBkZWZhdWx0IGRlZmluZUNvbmZpZyh7XG4gIHBsdWdpbnM6IFtyZWFjdCgpXSxcbiAgZGVmaW5lOiB7XG4gICAgX19BUFBfVkVSU0lPTl9fOiBKU09OLnN0cmluZ2lmeShwa2cudmVyc2lvbilcbiAgfSxcbiAgcmVzb2x2ZToge1xuICAgIGFsaWFzOiB7XG4gICAgICAnQCc6IHBhdGgucmVzb2x2ZShfX2Rpcm5hbWUsICdzcmMnKSxcbiAgICAgICdAc2hhcmVkJzogcGF0aC5yZXNvbHZlKF9fZGlybmFtZSwgJ3NyYy9zaGFyZWQnKSxcbiAgICAgICdAYnJhbmQnOiBwYXRoLnJlc29sdmUoX19kaXJuYW1lLCAnZG9jcycpXG4gICAgfVxuICB9LFxuICB0ZXN0OiB7XG4gICAgZW52aXJvbm1lbnQ6ICdqc2RvbScsXG4gICAgZ2xvYmFsczogdHJ1ZSxcbiAgICBzZXR1cEZpbGVzOiBbJ3NyYy90ZXN0L3NldHVwLnRzJ10sXG4gICAgLy8gUGl4aS94dGVybS9Nb25hY28gbGVhdmUgdGltZXJzICsgUkFGIGxvb3BzIGJlaGluZDsgdGhleSBhcmUgbW9ja2VkIHBlclxuICAgIC8vIHRlc3QgZmlsZSBhbnl3YXksIGFuZCB0aGlzIGtlZXBzIHRoZSBzdWl0ZSBmcm9tIGhhbmdpbmcgdGhlIHJ1bi5cbiAgICB0ZWFyZG93blRpbWVvdXQ6IDUwMDBcbiAgfSxcbiAgYXNzZXRzSW5jbHVkZTogWycqKi8qLnRtaiddXG59KTtcbiIsICJ7XG4gIFwibmFtZVwiOiBcImFnZW50LW9mZmljZVwiLFxuICBcInZlcnNpb25cIjogXCIwLjEuMFwiLFxuICBcInByaXZhdGVcIjogdHJ1ZSxcbiAgXCJkZXNjcmlwdGlvblwiOiBcIk11bHRpLWFnZW50IG9mZmljZSBoYXJuZXNzIFx1MjAxNCBUYXVyaSAoUnVzdCkgcG9ydCBvZiB0aGUgRWxlY3Ryb24gYXBwXCIsXG4gIFwidHlwZVwiOiBcIm1vZHVsZVwiLFxuICBcInNjcmlwdHNcIjoge1xuICAgIFwiZGV2XCI6IFwidml0ZVwiLFxuICAgIFwiYnVpbGRcIjogXCJ0c2MgLS1ub0VtaXQgJiYgdml0ZSBidWlsZFwiLFxuICAgIFwidGVzdFwiOiBcInZpdGVzdCBydW4gJiYgY2FyZ28gdGVzdCAtLW1hbmlmZXN0LXBhdGggc3JjLXRhdXJpL0NhcmdvLnRvbWxcIixcbiAgICBcInRlc3Q6dWlcIjogXCJ2aXRlc3QgcnVuXCIsXG4gICAgXCJ0ZXN0OnJzXCI6IFwiY2FyZ28gdGVzdCAtLW1hbmlmZXN0LXBhdGggc3JjLXRhdXJpL0NhcmdvLnRvbWxcIixcbiAgICBcInRhdXJpXCI6IFwidGF1cmlcIixcbiAgICBcInRhdXJpOmRldlwiOiBcInRhdXJpIGRldlwiLFxuICAgIFwidGF1cmk6YnVpbGRcIjogXCJ0YXVyaSBidWlsZFwiXG4gIH0sXG4gIFwiZGVwZW5kZW5jaWVzXCI6IHtcbiAgICBcIkBjb2RlbWlycm9yL2xhbmctY3NzXCI6IFwiXjYuMy4xXCIsXG4gICAgXCJAY29kZW1pcnJvci9sYW5nLWh0bWxcIjogXCJeNi40LjEyXCIsXG4gICAgXCJAY29kZW1pcnJvci9sYW5nLWphdmFzY3JpcHRcIjogXCJeNi4yLjVcIixcbiAgICBcIkBjb2RlbWlycm9yL2xhbmctanNvblwiOiBcIl42LjAuMlwiLFxuICAgIFwiQGNvZGVtaXJyb3IvbGFuZy1tYXJrZG93blwiOiBcIl42LjUuMlwiLFxuICAgIFwiQGNvZGVtaXJyb3IvbGFuZy1weXRob25cIjogXCJeNi4yLjFcIixcbiAgICBcIkBjb2RlbWlycm9yL2xhbmcteWFtbFwiOiBcIl42LjEuM1wiLFxuICAgIFwiQGNvZGVtaXJyb3IvbGFuZ3VhZ2VcIjogXCJeNi4xMi40XCIsXG4gICAgXCJAY29kZW1pcnJvci9zdGF0ZVwiOiBcIl42LjcuNlwiLFxuICAgIFwiQGNvZGVtaXJyb3Ivdmlld1wiOiBcIl42LjQzLjEzXCIsXG4gICAgXCJAbGV6ZXIvaGlnaGxpZ2h0XCI6IFwiXjEuMi41XCIsXG4gICAgXCJAbW9uYWNvLWVkaXRvci9yZWFjdFwiOiBcIl40LjcuMFwiLFxuICAgIFwiQG9wZW5haS9hZ2VudHMtcmVhbHRpbWVcIjogXCJeMC4xMS44XCIsXG4gICAgXCJAdGF1cmktYXBwcy9hcGlcIjogXCJeMi45LjBcIixcbiAgICBcIkB1aXcvcmVhY3QtY29kZW1pcnJvclwiOiBcIl40LjI1LjEyXCIsXG4gICAgXCJAeHRlcm0vYWRkb24tZml0XCI6IFwiXjAuMTAuMFwiLFxuICAgIFwiQHh0ZXJtL2FkZG9uLXVuaWNvZGUxMVwiOiBcIl4wLjkuMFwiLFxuICAgIFwiQHh0ZXJtL2FkZG9uLXdlYmdsXCI6IFwiXjAuMTkuMFwiLFxuICAgIFwiQHh0ZXJtL3h0ZXJtXCI6IFwiXjUuNS4wXCIsXG4gICAgXCJpMThuZXh0XCI6IFwiXjI2LjQuMlwiLFxuICAgIFwiaTE4bmV4dC1icm93c2VyLWxhbmd1YWdlZGV0ZWN0b3JcIjogXCJeOC4yLjFcIixcbiAgICBcIm1vbmFjby1lZGl0b3JcIjogXCJeMC41Mi4yXCIsXG4gICAgXCJwaXhpLmpzXCI6IFwiXjguMjIuMFwiLFxuICAgIFwicmVhY3RcIjogXCJeMTguMy4xXCIsXG4gICAgXCJyZWFjdC1kb21cIjogXCJeMTguMy4xXCIsXG4gICAgXCJyZWFjdC1pMThuZXh0XCI6IFwiXjE3LjAuMTVcIixcbiAgICBcInJlYWN0LW1hcmtkb3duXCI6IFwiXjEwLjEuMFwiLFxuICAgIFwicmVtYXJrLWdmbVwiOiBcIl40LjAuMVwiLFxuICAgIFwienVzdGFuZFwiOiBcIl40LjUuNVwiXG4gIH0sXG4gIFwiZGV2RGVwZW5kZW5jaWVzXCI6IHtcbiAgICBcIkB0YXVyaS1hcHBzL2NsaVwiOiBcIl4yLjkuMFwiLFxuICAgIFwiQHRlc3RpbmctbGlicmFyeS9kb21cIjogXCJeMTAuNC4yXCIsXG4gICAgXCJAdGVzdGluZy1saWJyYXJ5L3JlYWN0XCI6IFwiXjE2LjMuM1wiLFxuICAgIFwiQHRlc3RpbmctbGlicmFyeS91c2VyLWV2ZW50XCI6IFwiXjE0LjYuN1wiLFxuICAgIFwiQHR5cGVzL3JlYWN0XCI6IFwiXjE4LjMuMFwiLFxuICAgIFwiQHR5cGVzL3JlYWN0LWRvbVwiOiBcIl4xOC4zLjBcIixcbiAgICBcIkB2aXRlanMvcGx1Z2luLXJlYWN0XCI6IFwiXjQuMy4yXCIsXG4gICAgXCJqc2RvbVwiOiBcIl4yOS4xLjFcIixcbiAgICBcInR5cGVzY3JpcHRcIjogXCJeNS42LjNcIixcbiAgICBcInZpdGVcIjogXCJeNS40LjhcIixcbiAgICBcInZpdGVzdFwiOiBcIl4zLjIuN1wiXG4gIH1cbn1cbiJdLAogICJtYXBwaW5ncyI6ICI7QUFBeVQsU0FBUyxvQkFBb0I7QUFDdFYsT0FBTyxXQUFXO0FBQ2xCLE9BQU8sVUFBVTs7O0FDRmpCO0FBQUEsRUFDRSxNQUFRO0FBQUEsRUFDUixTQUFXO0FBQUEsRUFDWCxTQUFXO0FBQUEsRUFDWCxhQUFlO0FBQUEsRUFDZixNQUFRO0FBQUEsRUFDUixTQUFXO0FBQUEsSUFDVCxLQUFPO0FBQUEsSUFDUCxPQUFTO0FBQUEsSUFDVCxNQUFRO0FBQUEsSUFDUixXQUFXO0FBQUEsSUFDWCxXQUFXO0FBQUEsSUFDWCxPQUFTO0FBQUEsSUFDVCxhQUFhO0FBQUEsSUFDYixlQUFlO0FBQUEsRUFDakI7QUFBQSxFQUNBLGNBQWdCO0FBQUEsSUFDZCx3QkFBd0I7QUFBQSxJQUN4Qix5QkFBeUI7QUFBQSxJQUN6QiwrQkFBK0I7QUFBQSxJQUMvQix5QkFBeUI7QUFBQSxJQUN6Qiw2QkFBNkI7QUFBQSxJQUM3QiwyQkFBMkI7QUFBQSxJQUMzQix5QkFBeUI7QUFBQSxJQUN6Qix3QkFBd0I7QUFBQSxJQUN4QixxQkFBcUI7QUFBQSxJQUNyQixvQkFBb0I7QUFBQSxJQUNwQixvQkFBb0I7QUFBQSxJQUNwQix3QkFBd0I7QUFBQSxJQUN4QiwyQkFBMkI7QUFBQSxJQUMzQixtQkFBbUI7QUFBQSxJQUNuQix5QkFBeUI7QUFBQSxJQUN6QixvQkFBb0I7QUFBQSxJQUNwQiwwQkFBMEI7QUFBQSxJQUMxQixzQkFBc0I7QUFBQSxJQUN0QixnQkFBZ0I7QUFBQSxJQUNoQixTQUFXO0FBQUEsSUFDWCxvQ0FBb0M7QUFBQSxJQUNwQyxpQkFBaUI7QUFBQSxJQUNqQixXQUFXO0FBQUEsSUFDWCxPQUFTO0FBQUEsSUFDVCxhQUFhO0FBQUEsSUFDYixpQkFBaUI7QUFBQSxJQUNqQixrQkFBa0I7QUFBQSxJQUNsQixjQUFjO0FBQUEsSUFDZCxTQUFXO0FBQUEsRUFDYjtBQUFBLEVBQ0EsaUJBQW1CO0FBQUEsSUFDakIsbUJBQW1CO0FBQUEsSUFDbkIsd0JBQXdCO0FBQUEsSUFDeEIsMEJBQTBCO0FBQUEsSUFDMUIsK0JBQStCO0FBQUEsSUFDL0IsZ0JBQWdCO0FBQUEsSUFDaEIsb0JBQW9CO0FBQUEsSUFDcEIsd0JBQXdCO0FBQUEsSUFDeEIsT0FBUztBQUFBLElBQ1QsWUFBYztBQUFBLElBQ2QsTUFBUTtBQUFBLElBQ1IsUUFBVTtBQUFBLEVBQ1o7QUFDRjs7O0FENURBLElBQU0sbUNBQW1DO0FBS3pDLElBQU8sd0JBQVEsYUFBYTtBQUFBLEVBQzFCLFNBQVMsQ0FBQyxNQUFNLENBQUM7QUFBQSxFQUNqQixRQUFRO0FBQUEsSUFDTixpQkFBaUIsS0FBSyxVQUFVLGdCQUFJLE9BQU87QUFBQSxFQUM3QztBQUFBLEVBQ0EsU0FBUztBQUFBLElBQ1AsT0FBTztBQUFBLE1BQ0wsS0FBSyxLQUFLLFFBQVEsa0NBQVcsS0FBSztBQUFBLE1BQ2xDLFdBQVcsS0FBSyxRQUFRLGtDQUFXLFlBQVk7QUFBQSxNQUMvQyxVQUFVLEtBQUssUUFBUSxrQ0FBVyxNQUFNO0FBQUEsSUFDMUM7QUFBQSxFQUNGO0FBQUEsRUFDQSxNQUFNO0FBQUEsSUFDSixhQUFhO0FBQUEsSUFDYixTQUFTO0FBQUEsSUFDVCxZQUFZLENBQUMsbUJBQW1CO0FBQUE7QUFBQTtBQUFBLElBR2hDLGlCQUFpQjtBQUFBLEVBQ25CO0FBQUEsRUFDQSxlQUFlLENBQUMsVUFBVTtBQUM1QixDQUFDOyIsCiAgIm5hbWVzIjogW10KfQo=
