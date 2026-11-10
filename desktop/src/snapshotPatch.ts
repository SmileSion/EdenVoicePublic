import type {CoreSnapshot} from './types';
export interface SnapshotPatch {baseRevision:number;revision:number;changes:Partial<CoreSnapshot>}
/** Keep unchanged arrays/objects stable for memoized room and chat views. */
export function applySnapshotPatch(current:CoreSnapshot, patch:SnapshotPatch):CoreSnapshot|null {
  if(patch.revision<=current.revision)return current;
  if(patch.baseRevision!==current.revision)return null;
  return {...current,...patch.changes,revision:patch.revision};
}
