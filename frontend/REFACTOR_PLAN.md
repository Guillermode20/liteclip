# Frontend Refactoring Plan

This document outlines the plan to refactor the frontend codebase to improve maintainability, performance, and user experience. The goal is to transition from a monolithic `App.svelte` to a modular, component-based architecture with a modern, centered layout.

## Phase 1: Foundation & Organization
**Goal:** Clean up the project structure and set up the groundwork for state management and services.

- [x] **Create Directory Structure**
    - Ensure `src/stores`, `src/services`, `src/types`, `src/utils` exist.
- [x] **Centralize Types**
    - Move interfaces like `VideoSegment`, `UserSettingsPayload`, `UpdateInfoPayload` from `App.svelte` and other files to `src/types/index.ts`.
- [x] **Extract API Services**
    - Create `src/services/api.ts` to handle all `fetch` calls (FFmpeg status, settings, updates).
    - Refactor `App.svelte` to use these service functions instead of raw `fetch`.
    - **Verified**: Build successful after refactoring.

## Phase 2: State Management
**Goal:** Remove prop drilling and complex state logic from `App.svelte` by introducing Svelte stores.

- [ ] **Settings Store**
    - Create `src/stores/settings.ts` to manage `userSettings`, `autoUpdateEnabled`, etc.
    - Implement `loadSettings` and `saveSettings` actions within the store.
- [ ] **FFmpeg Store**
    - Create `src/stores/ffmpeg.ts` to manage `ffmpegReady`, `ffmpegStatusMessage`, `ffmpegProgressPercent`.
    - Move polling logic into this store.
- [ ] **File/Video Store**
    - Create `src/stores/video.ts` to manage `selectedFile`, `videoSegments`, `objectUrl`, and metadata.

## Phase 3: Component Extraction
**Goal:** Break down `App.svelte` into smaller, focused components.

- [ ] **Extract Header/Banner**
    - Create `components/Header.svelte` (Logo, Update Banner).
- [ ] **Extract Video Input**
    - Refactor `components/UploadArea.svelte` to use the `video` store.
- [ ] **Extract Output Controls**
    - Create `components/OutputControls.svelte` for the slider, codec selection, and resolution options.
- [ ] **Extract Advanced Options**
    - Create `components/AdvancedOptions.svelte` for the "Advanced Options" toggle section.
- [ ] **Extract Action Buttons**
    - Create `components/ActionButtons.svelte` for the "Compress Video" button and status messages.

## Phase 4: UI/UX Overhaul (Centered Layout)
**Goal:** Implement the new user-friendly, centered design.

- [ ] **Create Layout Component**
    - Create `components/Layout.svelte` to handle the main page structure (centered container).
- [ ] **Refactor Sidebar to Settings Panel**
    - Move the sidebar content into a `SettingsPanel` component that can be toggled or displayed as a modal/drawer.
    - Ensure it uses the `settings` store.
- [ ] **Update App.svelte**
    - Recompose `App.svelte` using `Layout`, `Header`, `UploadArea`, `OutputControls`, and `ActionButtons`.
    - Remove the old sidebar layout logic.

## Phase 5: Styling & Polish
**Goal:** Modernize the look and feel and clean up CSS.

- [ ] **Modularize CSS**
    - Move global styles from `app.css` to relevant components or a smaller `global.css`.
    - Use CSS variables for theme colors and spacing.
- [ ] **Visual Polish**
    - Apply the "premium" aesthetic (gradients, glassmorphism, smooth transitions) as requested.
    - Ensure responsive design for smaller screens.
