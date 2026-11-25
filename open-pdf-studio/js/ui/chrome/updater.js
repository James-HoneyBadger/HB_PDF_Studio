/** HB PDF Studio uses manual, user-initiated release management.
 * Never contact or install packages from the upstream project's updater.
 */
import { isTauri } from '../../core/platform.js';

export async function checkForUpdates(silent = true) {
  if (!isTauri() || silent) return;
  await window.__TAURI__?.dialog?.message?.(
    'Automatic updates are disabled for HB PDF Studio. Check the HB PDF Studio GitHub repository for Linux AArch64 releases.',
    { title: 'Software Update', kind: 'info' }
  );
}
