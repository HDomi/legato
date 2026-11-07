import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Tauri 개발 서버는 고정 포트를 쓴다 (tauri.conf.json 의 devUrl 과 맞춘다).
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1430, strictPort: true },
  build: { target: 'es2022', outDir: 'dist', emptyOutDir: true },
});
