/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_WSF_API_BASE?: string;
  readonly VITE_AOG_BASE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
