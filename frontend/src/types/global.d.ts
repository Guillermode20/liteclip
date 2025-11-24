export interface LiteClipAPI {
  showFolderDialog(): Promise<{ canceled: boolean; filePath?: string } | null>;
  resolveFolderDialog: ((result: string) => void) | null;
}

declare global {
  interface Window {
    liteclip?: LiteClipAPI;
  }
}
