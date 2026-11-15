import { createSignal, onMount, Show } from 'solid-js';
import { useTranslation } from '../../../i18n/useTranslation.js';
import { openExtern } from '../../../core/extern-openen.js';
import { defaultPdfAppStatus, invoke } from '../../../core/platform.js';
import { huidigeAppTekst, platformUit, zetAlsStandaard } from '../../../core/standaard-pdf-app.js';
import { showMessage } from '../../stores/dialogStore.js';
import { GEEN_DRAAGBAAR, leesDraagbaar } from '../../../core/draagbaar.js';

export default function FileAssocTab() {
  const { t } = useTranslation('preferences');
  const { t: tCommon } = useTranslation('common');
  const [currentApp, setCurrentApp] = createSignal(tCommon('checking'));

  // Draagbaar (#881): niet bij Windows geregistreerd; uitleg in plaats van
  // de knop en de miniaturen.
  const [draagbaar, setDraagbaar] = createSignal({ ...GEEN_DRAAGBAAR });

  onMount(() => {
    checkDefaultPdfApp();
    leesDraagbaar().then(setDraagbaar);
  });

  // Huidige standaard en "Als standaard instellen" via de Rust-kant
  // (standaard-pdf-app.js). In een Flatpak/Snap of zonder xdg-mime is het
  // niet na te gaan of te zetten; dan uitleg in plaats van stil falen.
  const platform = platformUit(navigator.platform || navigator.userAgent);
  let status = null;

  async function checkDefaultPdfApp() {
    status = await defaultPdfAppStatus();
    setCurrentApp(huidigeAppTekst(status, { t, tCommon, platform }));
  }

  async function handleSetDefault() {
    if (platform === 'windows') {
      try {
        await openExtern('ms-settings:defaultapps');
        showMessage(t('fileAssoc.windowsInstructions'));
      } catch {
        showMessage(t('fileAssoc.windowsFailed'));
      }
      return;
    }
    if (!status) status = await defaultPdfAppStatus();
    const melding = await zetAlsStandaard({ invoke, status, platform });
    if (melding) showMessage(t(melding.sleutel, melding.opties));
    // Na een gelukte xdg-mime default staat de nieuwe standaard er meteen.
    if (melding?.sleutel === 'fileAssoc.linuxDone') checkDefaultPdfApp();
  }

  return (
    <>
    <fieldset class="pref-fieldset">
      <legend>{t('fileAssoc.defaultPdfApplication')}</legend>
      <div class="pref-row">
        <label>{t('fileAssoc.currentDefault')}</label>
        <span style="font-size:11px;color:var(--theme-text-secondary, #666);">{currentApp()}</span>
      </div>
      <Show when={!draagbaar().portable} fallback={
        <p data-testid="prefs-portable-fileassoc" style="font-size:10px;color:var(--theme-text-secondary, #888);margin-top:12px;line-height:1.4;">
          {tCommon('portable.fileAssoc')}
        </p>
      }>
        <div class="pref-row" style="margin-top:12px;">
          <button type="button" class="pref-btn pref-btn-secondary" style="width:100%;" onClick={handleSetDefault}>
            {t('fileAssoc.setDefault')}
          </button>
        </div>
        <p style="font-size:10px;color:var(--theme-text-secondary, #888);margin-top:12px;line-height:1.4;">
          {t('fileAssoc.setDefaultHelp')}
        </p>
      </Show>
    </fieldset>
    </>
  );
}
