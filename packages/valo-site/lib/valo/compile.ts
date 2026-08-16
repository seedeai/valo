import { valoModule, type SceneModule } from './scene';

/**
 * Evaluate a scene module.
 *
 * A demo is written as a real ES module — it imports from `valo-web/raw` and
 * default-exports a `draw` function — and a card and the playground evaluate
 * the same text, so what the editor opens is provably what the card ran.
 *
 * The browser cannot `import` a string, so the two module keywords are
 * rewritten into the equivalent expressions: an import from `valo-web/raw`
 * becomes a destructure of the module object already in hand, and the exports
 * become a returned record. Nothing else is touched, and nothing is injected —
 * a name a scene did not import is a name it does not have.
 */

export type CompileResult =
  | { readonly ok: true; readonly module: SceneModule }
  | { readonly ok: false; readonly error: string };

const RAW_SPECIFIER = 'valo-web/raw';

/** `import { a, b as c } from '...'` — the only import form a scene needs. */
const IMPORT = /^[ \t]*import\s+(type\s+)?(\{[^}]*\}|[A-Za-z_$][\w$]*)\s+from\s+['"]([^'"]+)['"];?[ \t]*$/gm;
const DEFAULT_EXPORT = /^[ \t]*export\s+default\s+(?=function\s+[A-Za-z_$])/m;
const NAMED_EXPORT = /^[ \t]*export\s+(?=(?:function|const|let|var|class)\s)/gm;

export function compileScene(source: string): CompileResult {
  try {
    return { ok: true, module: evaluate(rewrite(source)) };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : String(error) };
  }
}

/**
 * Turn module syntax into statements `new Function` accepts.
 *
 * Line numbers are preserved: every replacement keeps its statement on the
 * line it started on, so a runtime error still points at the line the editor
 * is showing.
 */
function rewrite(source: string): string {
  const declared = source.match(/export\s+default\s+function\s+([A-Za-z_$][\w$]*)/);
  if (!declared) {
    throw new Error(
      'A scene must default-export its draw function: `export default function draw(scene) { … }`',
    );
  }
  const defaultName = declared[1];

  let body = source.replace(IMPORT, (statement, typeOnly, bindings, specifier) => {
    // A type-only import has no runtime meaning; erasing it is what a compiler
    // would do, and it is how a scene names the `Scene` it is handed.
    if (typeOnly) return '';
    if (specifier !== RAW_SPECIFIER) {
      throw new Error(
        `A scene can only import from '${RAW_SPECIFIER}'. Change '${specifier}', or drop the import.`,
      );
    }
    if (!bindings.startsWith('{')) {
      throw new Error(`'${RAW_SPECIFIER}' has no default export. Import the names you need: ${statement.trim()}`);
    }
    return `const ${bindings.replace(/\bas\b/g, ':')} = valo;`;
  });

  body = body.replace(DEFAULT_EXPORT, '');
  body = body.replace(NAMED_EXPORT, '');

  return `${body}\nreturn { default: ${defaultName}, dispose: typeof dispose === 'function' ? dispose : undefined };`;
}

function evaluate(body: string): SceneModule {
  // eslint-disable-next-line no-new-func -- a playground runs what the visitor typed; that is the feature.
  const factory = new Function('valo', `"use strict";\n${body}`) as (valo: unknown) => SceneModule;
  const module = factory(valoModule);
  if (typeof module.default !== 'function') {
    throw new Error('A scene must default-export a function.');
  }
  return module;
}
