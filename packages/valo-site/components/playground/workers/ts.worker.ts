/**
 * Monaco's TypeScript language worker, as a local entry point.
 *
 * `new Worker(new URL(...))` only resolves RELATIVE specifiers, so a bare
 * `monaco-editor/...` path inside the URL never reaches the bundler. Re-exporting
 * it from a file of our own gives the URL something relative to point at, and
 * the bundler emits the worker as its own chunk.
 */
import 'monaco-editor/languages/features/typescript/ts.worker';
