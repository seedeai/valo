'use client';

import { Editor, loader } from '@monaco-editor/react';
// The barrel registers every language Monaco ships. Importing the editor API
// and only the modules these two files need keeps this route's chunk to the
// editor plus two grammars, rather than eighty. JavaScript is a separate
// registration from TypeScript, and without it `scene.js` is plain text: no
// highlighting, and — silently — no diagnostics at all.
import * as monaco from 'monaco-editor/editor/editor.api';
import 'monaco-editor/languages/definitions/javascript/register';
import 'monaco-editor/languages/definitions/typescript/register';
import * as typescript from 'monaco-editor/languages/features/typescript/register';
import { useCallback } from 'react';
import { DIST_TYPES } from './scene-types';

/**
 * Monaco, self-hosted and confined to this route.
 *
 * This module is reached only through `next/dynamic` from the playground, so
 * the several megabytes below never enter the landing page's graph. It is also
 * the reason the editor is worth having: fed `@valo/web`'s own `.d.ts`, typing
 * `Paint.` lists the real API, which is a better argument for the
 * compatibility claim than any prose about it.
 */

loader.config({ monaco });

let workersConfigured = false;
let typesLoaded: Promise<void> | undefined;

/** Where the setup sample lives in the editor's virtual file system. */
export const SETUP_PATH = 'file:///setup.ts';
/** Its display copy: a second URI, so the tab does not edit the module `./setup` resolves to. */
export const SETUP_PREVIEW_PATH = 'file:///setup.preview.ts';
export const SCENE_PATH = 'file:///scene.js';

/**
 * Monaco's language services run in workers. Bundlers that understand
 * `new Worker(new URL(...))` — webpack and Turbopack both do — emit each one
 * as its own chunk, which keeps them out of the initial payload too.
 */
function configureWorkers() {
  if (workersConfigured) return;
  workersConfigured = true;
  self.MonacoEnvironment = {
    getWorker(_workerId, label) {
      if (label === 'typescript' || label === 'javascript') {
        return new Worker(new URL('./workers/ts.worker.ts', import.meta.url), { type: 'module' });
      }
      return new Worker(new URL('./workers/editor.worker.ts', import.meta.url), { type: 'module' });
    },
  };
}

const compilerOptions = {
  target: typescript.ScriptTarget.ESNext,
  module: typescript.ModuleKind.ESNext,
  moduleResolution: typescript.ModuleResolutionKind.NodeJs,
  // No `lib` override. Monaco derives the standard library from `target`, and
  // naming libs here that its worker does not recognise drops them all — which
  // shows up as `Float32Array` and `Array.length` being unknown.
  allowJs: true,
  // Scenes are JavaScript with a JSDoc type on the one parameter they take, so
  // the checker has to read JavaScript for the editor to know what `scene` is.
  checkJs: true,
  strict: false,
  // Scene bodies carry untyped local helpers. Demand annotations on those and
  // every scene grows noise that teaches nothing.
  noImplicitAny: false,
  baseUrl: 'file:///',
  paths: {
    '@valo/web': ['file:///node_modules/@valo/web/dist/index.d.ts'],
    '@valo/web/raw': ['file:///node_modules/@valo/web/dist/raw.d.ts'],
  },
};

const diagnosticsOptions = { noSemanticValidation: false, noSyntaxValidation: false };

/**
 * Mount the package's shipped declarations, and the setup sample, in the
 * editor's virtual file system.
 *
 * The layout mirrors the package on disk, because `dist/raw.d.ts` imports
 * `../wasm/valo_web.js` and that relative path has to land somewhere real.
 * `setup.ts` sits at the root beside `scene.js`, so the scene's
 * `import('./setup')` resolves to the same file the setup tab is showing.
 */
async function loadValoTypes() {
  for (const defaults of [typescript.typescriptDefaults, typescript.javascriptDefaults]) {
    defaults.setCompilerOptions(compilerOptions);
    defaults.setDiagnosticsOptions(diagnosticsOptions);
  }

  const files = await Promise.all(
    [...DIST_TYPES.map((name) => `dist/${name}`), 'wasm/valo_web'].map(async (name) => ({
      path: `file:///node_modules/@valo/web/${name}.d.ts`,
      text: await (await fetch(`/valo/types/${name.split('/').pop()}.d.ts`)).text(),
    })),
  );
  const libraries = [...files, { text: await fetchSetupSource(), path: SETUP_PATH }];
  // Each language's defaults keep their OWN extra libraries, and a scene is
  // JavaScript while the declarations it needs are TypeScript. Registering on
  // one only is silent: nothing resolves and no diagnostic says why.
  for (const defaults of [typescript.typescriptDefaults, typescript.javascriptDefaults]) {
    for (const library of libraries) defaults.addExtraLib(library.text, library.path);
  }
}

let setupSource: Promise<string> | undefined;

/** The bootstrap sample, served as the file it is. Staged by `copy-valo-assets.mjs`. */
export function fetchSetupSource(): Promise<string> {
  setupSource ??= fetch('/valo/setup.ts').then((response) => response.text());
  return setupSource;
}

function defineValoTheme() {
  monaco.editor.defineTheme('valo', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'comment', foreground: '5f5e65', fontStyle: 'italic' },
      { token: 'keyword', foreground: 'c8ff3d' },
      { token: 'number', foreground: 'a78bff' },
      { token: 'string', foreground: 'ff8fa8' },
      { token: 'identifier', foreground: 'f4f2ef' },
    ],
    colors: {
      'editor.background': '#0e0e12',
      'editor.foreground': '#f4f2ef',
      'editorLineNumber.foreground': '#3a3940',
      'editorLineNumber.activeForeground': '#8f8e94',
      'editor.selectionBackground': '#c8ff3d24',
      'editor.lineHighlightBackground': '#ffffff08',
      'editorCursor.foreground': '#c8ff3d',
      'editorIndentGuide.background1': '#ffffff10',
    },
  });
}

export interface SceneEditorProps {
  readonly value: string;
  readonly path: string;
  readonly language: 'javascript' | 'typescript';
  readonly readOnly?: boolean;
  readonly onChange?: (value: string) => void;
}

export default function SceneEditor({
  value,
  path,
  language,
  readOnly = false,
  onChange,
}: SceneEditorProps) {
  const handleMount = useCallback(() => {
    defineValoTheme();
    monaco.editor.setTheme('valo');
  }, []);

  configureWorkers();
  typesLoaded ??= loadValoTypes();

  return (
    <Editor
      value={value}
      onChange={(next) => onChange?.(next ?? '')}
      language={language}
      path={path}
      theme="valo"
      onMount={handleMount}
      loading={
        <span className="font-mono text-[10px] uppercase tracking-[0.13em] text-white/25">
          loading editor
        </span>
      }
      options={{
        readOnly,
        domReadOnly: readOnly,
        fontSize: 13,
        fontFamily: 'var(--font-dm-mono), ui-monospace, monospace',
        lineHeight: 1.7,
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        smoothScrolling: true,
        padding: { top: 20, bottom: 20 },
        renderLineHighlight: readOnly ? 'none' : 'line',
        tabSize: 2,
        automaticLayout: true,
        wordWrap: 'on',
        overviewRulerLanes: 0,
        scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8 },
      }}
    />
  );
}
