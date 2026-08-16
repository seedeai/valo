import * as valoRaw from '@valo/web/raw';

export type { Scene, SceneModule } from './standalone-setup';

/** The raw valo module exactly as `@valo/web/raw` exports it. */
export type ValoModule = typeof valoRaw;

export const valoModule: ValoModule = valoRaw;
