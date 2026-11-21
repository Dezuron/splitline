<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { toast } from "../lib/toast";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import type { XraySiteBundle, XraySiteCandidate } from "../types";
import {
  AlertTriangle,
  Check,
  Copy,
  Download,
  RefreshCw,
  RotateCw,
  Send,
  ShieldCheck,
  Zap,
} from "lucide-vue-next";
import Card from "../components/ui/card/Card.vue";
import CardContent from "../components/ui/card/CardContent.vue";
import CardDescription from "../components/ui/card/CardDescription.vue";
import CardHeader from "../components/ui/card/CardHeader.vue";
import CardTitle from "../components/ui/card/CardTitle.vue";
import Button from "../components/ui/button/Button.vue";
import Toggle from "../components/Toggle.vue";
import { formatBytes } from "../lib/labels";
import {
  config,
  copyXrayLink,
  formatAgo,
  installXrayCore,
  installXrayCoreFromFile,
  loadXrayProfiles,
  openXrayInTelegram,
  openReleasePage,
  pingXrayProfiles,
  refreshXray,
  refreshXraySubscription,
  restartXray,
  restorePreviousSubscription,
  selectXrayProfile,
  setXrayPorts,
  setXrayAutoSwitch,
  setXrayProxySites,
  setXraySubscription,
  setXraySystemProxy,
  status,
  discoverXraySites,
  exportXraySites,
  getXraySiteBundles,
  importXraySites,
  tgwsRunning,
  toggleXray,
  updateXraySiteBases,
  xrayBusy,
  xrayError,
  xrayLastTrafficAt,
  xrayLink,
  xrayLogs,
  xrayProfiles,
  xrayStatus,
  vpnStatusText,
} from "../store";

const urlDraft = ref("");
const socksDraft = ref(17808);
const httpDraft = ref(17809);
const siteDraft = ref("");
const siteBundles = ref<XraySiteBundle[]>([]);
const groupPick = ref("");
const suggestions = ref<(XraySiteCandidate & { checked: boolean })[]>([]);
const suggestionFor = ref("");
const discovering = ref(false);
const basesBusy = ref(false);
const logsOpen = ref(false);
const logBox = ref<HTMLElement | null>(null);

const running = computed(() => xrayStatus.value?.state === "running");
const telegramConnected = computed(() => (xrayStatus.value?.client_connections ?? 0) > 0);
/** The last byte through the tunnel: "—" until traffic is confirmed. */
const vpnLastActivityLabel = computed(() => formatAgo(xrayLastTrafficAt.value));
/** Telegram keeps only one proxy at a time, so an active MTProto proxy wins over the VPN. */
const telegramOnProxy = computed(() => tgwsRunning.value && !telegramConnected.value);
const transitioning = computed(
  () => xrayStatus.value?.state === "starting" || xrayStatus.value?.state === "stopping",
);
const canConnect = computed(() => !!config.value?.xray.subscription_url?.trim());

/** Zapret, not the VPN, is what fixes Discord voice: it carries raw UDP. */
const zapretActive = computed(() => !!status.value?.runtime_valid && !!status.value?.running);

const selectedBundle = computed(
  () => siteBundles.value.find((bundle) => bundle.name === groupPick.value) ?? null,
);
const profilesSorted = computed(() =>
  [...xrayProfiles.value].sort((a, b) => {
    if (a.ping_ms === null && b.ping_ms === null) return a.index - b.index;
    if (a.ping_ms === null) return 1;
    if (b.ping_ms === null) return -1;
    return a.ping_ms - b.ping_ms;
  }),
);
const logText = computed(() => xrayLogs.value.join("\n"));
const selectedPing = computed(() => {
  const index = xrayStatus.value?.selected;
  if (index === undefined) return "—";
  const profile = xrayProfiles.value.find((item) => item.index === index);
  return profile?.ping_ms == null ? "—" : `${profile.ping_ms} мс`;
});
const subscriptionLabel = computed(() => {
  const status = xrayStatus.value;
  if (!status?.subscription_total) return "—";
  const used = (status.subscription_upload ?? 0) + (status.subscription_download ?? 0);
  const left = Math.max(0, status.subscription_total - used);
  const expire = status.subscription_expire ?? 0;
  let until = "";
  if (expire > 1_000_000_000) {
    until = ` · до ${new Date(expire * 1000).toLocaleDateString("ru-RU")}`;
  } else if (expire > 0) {
    until = ` · ${Math.ceil(expire / 86_400)} дн.`;
  }
  return `${formatBytes(left)} из ${formatBytes(status.subscription_total)}${until}`;
});

watch(
  () => config.value?.xray,
  (xray) => {
    if (!xray) return;
    urlDraft.value = xray.subscription_url;
    socksDraft.value = xray.socks_port;
    httpDraft.value = xray.http_port;
  },
  { immediate: true, deep: true },
);

watch(logText, async () => {
  if (!logsOpen.value) return;
  await nextTick();
  logBox.value?.scrollTo({ top: logBox.value.scrollHeight });
});

onMounted(async () => {
  await refreshXray();
  try {
    siteBundles.value = await getXraySiteBundles();
  } catch (error) {
    console.warn("Не удалось получить наборы сайтов", error);
  }
  if (config.value?.xray.subscription_url && !xrayProfiles.value.length) {
    // First visit: ping every location in parallel so the list is not "n/a".
    await loadXrayProfiles(true);
  }
});

async function commitUrl() {
  const value = urlDraft.value.trim();
  if (value === (config.value?.xray.subscription_url ?? "")) return;
  try {
    urlDraft.value = await setXraySubscription(value);
    await refreshXraySubscription(false);
  } catch {
    urlDraft.value = config.value?.xray.subscription_url ?? "";
  }
}

async function commitPorts() {
  const socks = Number(socksDraft.value);
  const http = Number(httpDraft.value);
  if (!Number.isFinite(socks) || !Number.isFinite(http)) return;
  try {
    const [savedSocks, savedHttp] = await setXrayPorts(Math.round(socks), Math.round(http));
    socksDraft.value = savedSocks;
    httpDraft.value = savedHttp;
  } catch {
    socksDraft.value = config.value?.xray.socks_port ?? 17808;
    httpDraft.value = config.value?.xray.http_port ?? 17809;
  }
}

async function pick(index: number) {
  if (xrayBusy.value) return;
  try {
    await selectXrayProfile(index);
  } catch {
    /* тост уже показан */
  }
}

const sites = computed(() => config.value?.xray.proxy_sites ?? []);

/** Notes of the bundles already in the list — shown when their traffic leaks. */
const bundleNotes = computed(() => {
  const notes: string[] = [];
  for (const site of sites.value) {
    const category = site.startsWith("geosite:") ? site.slice("geosite:".length) : "";
    const bundle = siteBundles.value.find((item) => item.category === category);
    if (bundle?.note) notes.push(bundle.note);
  }
  return notes;
});

/** "Базы от 18.11.2026" — empty until the first update. */
const basesDate = computed(() => {
  const stamp = config.value?.xray.bases_updated_at ?? 0;
  if (!stamp) return "";
  return new Date(stamp * 1000).toLocaleDateString("ru-RU");
});

function prettySite(site: string): string {
  if (site.startsWith("geosite:")) {
    const category = site.slice("geosite:".length);
    const bundle = siteBundles.value.find((item) => item.category === category);
    // `meta` covers Instagram and its CDN, so the bundle name is what matters.
    return `${bundle ? bundle.name : category} · набор`;
  }
  return site.replace(/^(domain|full|regexp):/, "");
}

async function saveSites(next: string[]): Promise<boolean> {
  try {
    await setXrayProxySites(next);
    return true;
  } catch {
    // The toast is already shown by the action.
    return false;
  }
}

async function addSite() {
  const value = siteDraft.value.trim();
  if (!value) return;
  // Only look for extra domains once the site is really in the list: a failed
  // save already explains itself, and a discovery error on top of it reads as
  // "the site was not added".
  if (!(await saveSites([...sites.value, value]))) return;
  siteDraft.value = "";
  // A site is rarely one host: ask the backend what else it loads.
  void discover(value);
}

/** Adds a bundle: one category instead of a pile of CDN domains. */
async function addBundle(bundle: XraySiteBundle) {
  await saveSites([...sites.value, `geosite:${bundle.category}`]);
  // Discord voice is UDP and never reaches the proxy: say so once, if zapret is
  // not running to cover it.
  if (bundle.note && !zapretActive.value) {
    toast("warning", bundle.note);
  }
}

async function removeSite(site: string) {
  await saveSites(sites.value.filter((item) => item !== site));
}

/** Adds the bundle chosen in the dropdown. */
async function addSelectedGroup() {
  const bundle = siteBundles.value.find((item) => item.name === groupPick.value);
  if (!bundle) return;
  await addBundle(bundle);
  groupPick.value = "";
}

/** Reloads the merged domain lists, then refreshes the bundle list. */
async function refreshBases() {
  if (basesBusy.value) return;
  basesBusy.value = true;
  try {
    const summary = await updateXraySiteBases();
    if (summary) {
      siteBundles.value = await getXraySiteBundles();
    }
  } finally {
    basesBusy.value = false;
  }
}

/** Asks the backend which extra hosts the site needs (CDN, static). */
async function discover(site: string) {
  discovering.value = true;
  suggestionFor.value = site;
  try {
    const candidates = await discoverXraySites(site);
    suggestions.value = candidates.map((candidate) => ({
      ...candidate,
      // The likely assets are pre-selected, everything else is opt-in.
      checked: candidate.strong,
    }));
  } catch (error) {
    suggestions.value = [];
    // The site itself is already saved; only the extra-domain lookup failed.
    toast("warning", `Сайт добавлен, но проверить дополнительные домены не удалось: ${error}`);
  } finally {
    discovering.value = false;
  }
}

async function acceptSuggestions() {
  const picked = suggestions.value.filter((item) => item.checked).map((item) => item.host);
  if (!picked.length) {
    suggestions.value = [];
    return;
  }
  await saveSites([...sites.value, ...picked]);
  suggestions.value = [];
}

/** Loads a domain list from a text file (one entry per line). */
async function importFromFile() {
  const picked = await openDialog({
    multiple: false,
    filters: [{ name: "Список доменов", extensions: ["txt", "list", "conf", "csv"] }],
  });
  if (!picked || Array.isArray(picked)) return;
  try {
    await importXraySites(picked);
    toast("success", "Список доменов загружен.");
  } catch (error) {
    toast("error", String(error));
  }
}

async function toggleSystemProxy(enabled: boolean) {
  try {
    await setXraySystemProxy(enabled);
  } catch {
    /* тост уже показан */
  }
}

/** Browsers are covered only while the tunnel is up and the proxy is applied. */
const sitesEnabled = computed(() => running.value && !!config.value?.xray.system_proxy);

/** The tunnel is shared: it carries Telegram and, when enabled, the browsers. */
async function setTunnel(enabled: boolean) {
  if (enabled === running.value) return;
  await toggleXray();
}

/** Turning sites on also raises the tunnel, so the switch is never dead. */
async function setSites(enabled: boolean) {
  if (enabled === sitesEnabled.value) return;
  if (enabled && !running.value) {
    await toggleXray();
    if (!running.value) return;
  }
  await toggleSystemProxy(enabled);
}

/** Installs a downloaded `Xray-windows-64.zip`: the way out when GitHub is blocked. */
async function installCoreFromFile() {
  const picked = await openDialog({
    multiple: false,
    filters: [{ name: "Архив Xray", extensions: ["zip"] }],
  });
  if (!picked || Array.isArray(picked)) return;
  await installXrayCoreFromFile(picked);
}

/** Saves the whitelist to a text file the import button can read back. */
async function exportToFile() {
  const target = await saveDialog({
    defaultPath: "splitline-domains.txt",
    filters: [{ name: "Список доменов", extensions: ["txt"] }],
  });
  if (!target) return;
  try {
    const count = await exportXraySites(target);
    toast("success", `Сохранено доменов: ${count}.`);
  } catch (error) {
    toast("error", String(error));
  }
}

function pingLabel(ms: number | null): string {
  if (ms === null) return "n/a";
  return `${ms} мс`;
}
</script>

<template>
  <div class="page-stack vpn-page">
    <Card class="vpn-card">
      <CardHeader class="vpn-hero-head">
        <div class="vpn-hero-icon"><Zap :size="21" /></div>
        <div class="vpn-hero-text">
          <span class="eyebrow">VPN</span>
          <CardTitle>Подписка</CardTitle>
          <CardDescription>{{ vpnStatusText }}</CardDescription>
        </div>
        <div class="vpn-hero-controls">
          <span
            class="vpn-dot"
            :class="{ ok: running, warn: transitioning, danger: xrayStatus?.state === 'error' }"
            aria-hidden="true"
          ></span>
        </div>
      </CardHeader>
      <CardContent class="vpn-hero-content">
        <div class="vpn-toggles">
          <Toggle
            label="VPN для Telegram"
            hint="Туннель для Telegram: один раз подтвердите прокси в приложении Telegram"
            :checked="running"
            :disabled="xrayBusy || transitioning || !canConnect"
            @change="setTunnel"
          />
          <Toggle
            label="VPN для сайтов"
            hint="Браузеры через системный прокси. Включение поднимет туннель, если он выключен"
            :checked="sitesEnabled"
            :disabled="xrayBusy || transitioning || !canConnect"
            @change="setSites"
          />
        </div>
        <div class="vpn-meta">
          <div>
            <span>Локация</span>
            <strong>{{ xrayStatus?.profile_name || "не выбрана" }}</strong>
          </div>
          <div>
            <span>SOCKS5</span>
            <strong>127.0.0.1:{{ xrayStatus?.socks_port ?? "—" }}</strong>
          </div>
          <div>
            <span>Ядро</span>
            <strong>{{
              xrayStatus?.installed_version ||
              (xrayStatus?.binary_present ? "установлено" : "не установлено")
            }}</strong>
          </div>
          <div>
            <span>Пинг</span>
            <strong>{{ selectedPing }}</strong>
          </div>
          <div>
            <span>Активность</span>
            <strong>{{ vpnLastActivityLabel }}</strong>
          </div>
          <div v-if="xrayStatus?.subscription_total">
            <span>Подписка</span>
            <strong>{{ subscriptionLabel }}</strong>
            <button
              v-if="xrayStatus?.subscription_page"
              class="vpn-page-link"
              type="button"
              @click="openReleasePage(xrayStatus.subscription_page)"
            >
              Личный кабинет
            </button>
          </div>
        </div>

        <label class="field field-wide">
          <span class="field-label">Ссылка на подписку</span>
          <input
            v-model="urlDraft"
            class="vpn-input vpn-mono"
            spellcheck="false"
            placeholder="https://provider.example/api/sub/…"
            @change="commitUrl"
            @keydown.enter="commitUrl"
          />
        </label>

        <div class="field field-wide">
          <span class="field-label">Ссылка для Telegram</span>
          <div class="vpn-link-row">
            <code class="vpn-link">{{ xrayLink || "ссылка появится после запуска" }}</code>
            <Button
              variant="outline"
              size="icon"
              title="Копировать ссылку"
              aria-label="Копировать ссылку для Telegram"
              :disabled="!xrayLink"
              @click="copyXrayLink"
            >
              <Copy :size="15" />
            </Button>
          </div>
        </div>

        <div class="vpn-actions">
          <Button
            size="sm"
            :disabled="xrayBusy || !canConnect"
            @click="refreshXraySubscription(true)"
          >
            <Download :size="14" />
            Обновить подписку
          </Button>
          <Button variant="outline" size="sm" :disabled="xrayBusy" @click="pingXrayProfiles">
            Замерить пинг
          </Button>
          <Button
            variant="outline"
            size="sm"
            :disabled="xrayBusy || transitioning || !running"
            @click="restartXray"
          >
            <RotateCw :size="14" />
            Перезапустить
          </Button>
          <Button
            v-if="!xrayStatus?.binary_present"
            size="sm"
            :disabled="xrayBusy"
            @click="installXrayCore"
          >
            <Download :size="14" />
            Скачать ядро Xray
          </Button>
          <Button
            v-if="!xrayStatus?.binary_present"
            variant="outline"
            size="sm"
            :disabled="xrayBusy"
            title="Если GitHub недоступен: скачайте Xray-windows-64.zip и укажите его"
            @click="installCoreFromFile"
          >
            Установить из файла
          </Button>
        </div>

        <p v-if="!canConnect" class="vpn-hint">
          Вставьте ссылку подписки — без неё подключаться нечем.
        </p>
        <div v-if="xrayStatus?.last_error" class="vpn-error">
          <AlertTriangle :size="14" />
          <span>{{ xrayStatus.last_error }}</span>
        </div>
        <div v-if="xrayError" class="vpn-error">
          <AlertTriangle :size="14" />
          <span>{{ xrayError }}</span>
        </div>

        <div v-if="running && !telegramConnected" class="vpn-telegram-prompt">
          <div class="vpn-hint-icon"><Send :size="18" /></div>
          <div v-if="telegramOnProxy" class="vpn-hint-text">
            <strong>Telegram сидит на MTProto-прокси, а не на VPN</strong>
            <span>
              В Telegram может быть активен только один прокси, поэтому трафик идёт через MTProto.
              Переключить Telegram на VPN?
            </span>
          </div>
          <div v-else class="vpn-hint-text">
            <strong>Осталось переключить Telegram на VPN</strong>
            <span
              >Нажми кнопку — Telegram откроет своё окно с готовым прокси, останется
              подтвердить.</span
            >
          </div>
          <Button size="sm" @click="openXrayInTelegram">
            {{ telegramOnProxy ? "Переключить на VPN" : "Подключить Telegram" }}
          </Button>
        </div>

        <div class="vpn-locations">
          <div class="vpn-locations-head">
            <strong>Локации</strong>
            <span>{{ xrayProfiles.length }} из подписки · пинг обновляется сам</span>
          </div>
          <p v-if="!xrayProfiles.length" class="vpn-hint">
            Список пуст — нажмите «Обновить подписку» после вставки ссылки.
          </p>
          <template v-else>
            <div class="vpn-list-head" aria-hidden="true">
              <span>Локация</span>
              <span>Протокол</span>
              <span>Пинг</span>
            </div>
            <div class="vpn-list">
              <button
                v-for="profile in profilesSorted"
                :key="profile.index"
                type="button"
                class="vpn-row"
                :class="{ selected: profile.index === (xrayStatus?.selected ?? -1) }"
                :aria-pressed="profile.index === (xrayStatus?.selected ?? -1)"
                :disabled="xrayBusy"
                @click="pick(profile.index)"
              >
                <span class="vpn-row-name">
                  <Check
                    v-if="profile.index === (xrayStatus?.selected ?? -1)"
                    class="vpn-row-check"
                    :size="14"
                    :stroke-width="2.6"
                    aria-hidden="true"
                  />
                  {{ profile.name }}
                </span>
                <span class="vpn-row-scheme">{{ profile.scheme }}</span>
                <span class="vpn-row-ping">{{ pingLabel(profile.ping_ms) }}</span>
              </button>
            </div>
          </template>
        </div>
      </CardContent>
    </Card>

    <Card v-if="running && config?.xray.system_proxy" class="vpn-card vpn-guard-card">
      <div class="vpn-hint-icon"><ShieldCheck :size="18" /></div>
      <div class="vpn-hint-text">
        <strong>Системный прокси под защитой</strong>
        <span>
          Если его снимет другое приложение или Windows, приложение вернёт настройку само.
        </span>
      </div>
    </Card>

    <Card v-if="running && !config?.xray.system_proxy" class="vpn-card vpn-hint-card">
      <div class="vpn-hint-icon"><Zap :size="18" /></div>
      <div class="vpn-hint-text">
        <strong>Браузеры идут мимо VPN</strong>
        <span>
          VPN включён, но системный прокси выключен — Chrome и другие браузеры открывают сайты
          напрямую. Включите «VPN для сайтов» выше, иначе заблокированные сайты не откроются.
        </span>
      </div>
      <Button size="sm" :disabled="xrayBusy" @click="toggleSystemProxy(true)">Включить</Button>
    </Card>

    <Card v-if="running && telegramConnected" class="vpn-card vpn-ok-card">
      <div class="vpn-hint-icon"><Send :size="18" /></div>
      <div class="vpn-hint-text">
        <strong>Telegram работает через VPN</strong>
        <span>
          Активных соединений: {{ xrayStatus?.client_connections }} через профиль «{{
            xrayStatus?.profile_name
          }}».
        </span>
      </div>
    </Card>

    <Card class="vpn-card">
      <CardHeader>
        <div>
          <CardTitle>Сайты через VPN</CardTitle>
          <CardDescription v-if="basesDate">
            Наборы берутся из баз доменов от {{ basesDate }}.
          </CardDescription>
          <CardDescription v-else>
            Наборы работают после загрузки баз — нажмите «Обновить базы».
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent class="vpn-fields">
        <div class="vpn-bundle-row field-wide">
          <template v-if="siteBundles.length">
            <span class="vpn-groups-label">Готовый набор:</span>
            <select v-model="groupPick" class="vpn-input vpn-select" :disabled="xrayBusy">
              <option value="">— выберите —</option>
              <option
                v-for="bundle in siteBundles"
                :key="bundle.category"
                :value="bundle.name"
                :disabled="!bundle.available"
              >
                {{ bundle.name }} · {{ bundle.available ? bundle.domains : "нет в базах" }}
              </option>
            </select>
            <Button size="sm" :disabled="xrayBusy || !groupPick" @click="addSelectedGroup">
              Добавить
            </Button>
          </template>
          <Button
            variant="outline"
            size="sm"
            class="vpn-bases-btn"
            :disabled="xrayBusy || basesBusy"
            @click="refreshBases"
          >
            <RefreshCw :size="14" />
            {{ basesBusy ? "Обновляю…" : "Обновить базы" }}
          </Button>
        </div>
        <p v-if="selectedBundle?.note" class="vpn-hint field-wide">{{ selectedBundle.note }}</p>

        <div class="vpn-site-row field-wide">
          <input
            v-model="siteDraft"
            class="vpn-input vpn-mono"
            spellcheck="false"
            placeholder="example.com или https://example.com/page"
            @keydown.enter="addSite"
          />
          <Button size="sm" :disabled="xrayBusy || !siteDraft.trim()" @click="addSite">
            Добавить
          </Button>
          <Button variant="outline" size="sm" :disabled="xrayBusy" @click="importFromFile">
            Загрузить из файла
          </Button>
          <Button
            variant="outline"
            size="sm"
            :disabled="xrayBusy || !sites.length"
            @click="exportToFile"
          >
            Выгрузить в файл
          </Button>
        </div>

        <div v-if="discovering" class="vpn-hint field-wide">Ищу, какие ещё домены нужны сайту…</div>
        <div v-else-if="suggestions.length" class="vpn-suggest field-wide">
          <div class="vpn-suggest-head">
            <strong>Для «{{ suggestionFor }}» страница ссылается на эти хосты</strong>
            <span>отметь нужные — частые обычно и есть картинки и стили</span>
          </div>
          <div class="vpn-suggest-list">
            <label v-for="item in suggestions" :key="item.host" class="vpn-suggest-row">
              <input v-model="item.checked" type="checkbox" :disabled="xrayBusy" />
              <code>{{ item.host }}</code>
              <span class="vpn-suggest-hits">{{ item.hits }}×</span>
            </label>
          </div>
          <div class="vpn-actions">
            <Button size="sm" :disabled="xrayBusy" @click="acceptSuggestions"
              >Добавить выбранные</Button
            >
            <Button variant="ghost" size="sm" @click="suggestions = []">Скрыть</Button>
          </div>
        </div>

        <details v-if="sites.length" class="vpn-details field-wide" open>
          <summary>Домены в списке: {{ sites.length }}</summary>
          <div class="vpn-sites">
            <span v-for="site in sites" :key="site" class="vpn-chip">
              {{ prettySite(site) }}
              <button
                type="button"
                class="vpn-chip-remove"
                :disabled="xrayBusy"
                :aria-label="`Убрать ${site}`"
                @click="removeSite(site)"
              >
                ×
              </button>
            </span>
          </div>
        </details>
        <p v-else class="vpn-hint field-wide">
          Список пуст — через VPN идёт только Telegram, остальное напрямую.
        </p>
        <p v-if="bundleNotes.length && !zapretActive" class="vpn-hint field-wide">
          {{ bundleNotes[0] }}
        </p>
      </CardContent>
    </Card>

    <Card class="vpn-card">
      <details class="vpn-advanced">
        <summary>
          <span>Дополнительно</span>
          <span class="muted">Порты, ядро и логи</span>
        </summary>
        <div class="vpn-advanced-body">
          <div class="vpn-fields">
            <label class="field">
              <span class="field-label">SOCKS5 порт</span>
              <input
                v-model.number="socksDraft"
                class="vpn-input"
                type="number"
                min="1024"
                max="65535"
                @change="commitPorts"
                @keydown.enter="commitPorts"
              />
            </label>
            <label class="field">
              <span class="field-label">HTTP порт</span>
              <input
                v-model.number="httpDraft"
                class="vpn-input"
                type="number"
                min="1024"
                max="65535"
                @change="commitPorts"
                @keydown.enter="commitPorts"
              />
            </label>
            <p class="vpn-hint field-wide">
              Порты локального прокси, к которому подключается Telegram. Смена перезапускает VPN.
              VPN включится при следующем запуске, если был включён при выходе.
            </p>
            <Toggle
              label="Автопереключение локаций"
              hint="Если туннель не отвечает 5 минут — переключиться на самую быструю живую локацию"
              :checked="config?.xray.auto_switch"
              :disabled="xrayBusy"
              @change="setXrayAutoSwitch"
            />
            <Button
              v-if="xrayStatus?.has_previous"
              variant="secondary"
              size="sm"
              class="self-start"
              :disabled="xrayBusy"
              @click="restorePreviousSubscription"
            >
              Вернуть предыдущую подписку
            </Button>
          </div>

          <div class="vpn-log-head">
            <div>
              <span class="field-label">Логи</span>
              <span class="muted">Последние {{ xrayLogs.length }} строк от ядра Xray</span>
            </div>
            <Button variant="ghost" size="sm" @click="logsOpen = !logsOpen">
              {{ logsOpen ? "Свернуть" : "Показать" }}
            </Button>
          </div>
          <pre v-if="logsOpen" ref="logBox" class="vpn-log">{{
            logText || "Пока пусто — подключите VPN."
          }}</pre>
        </div>
      </details>
    </Card>
  </div>
</template>

<style scoped>
.vpn-page {
  padding-bottom: 8px;
}

.vpn-card {
  border-color: var(--glass-border);
  background: linear-gradient(160deg, var(--glass-bg), var(--glass-bg-2));
  box-shadow:
    0 18px 50px var(--glass-shadow),
    inset 0 1px 0 var(--glass-highlight);
  backdrop-filter: blur(18px) saturate(135%);
}

.vpn-hero-head {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 12px;
}

.vpn-hero-icon {
  display: grid;
  place-items: center;
  width: 40px;
  height: 40px;
  flex: 0 0 auto;
  border-radius: 12px;
  border: 1px solid var(--glass-border);
  background: var(--accent-soft);
  color: var(--accent-text);
}

.vpn-hero-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.vpn-hero-controls {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-left: auto;
}

.vpn-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-3);
}

.vpn-dot.ok {
  background: var(--ok);
  box-shadow: 0 0 0 3px var(--ok-soft);
}

.vpn-dot.warn {
  background: var(--warn);
  box-shadow: 0 0 0 3px var(--warn-soft);
}

.vpn-dot.danger {
  background: var(--danger);
  box-shadow: 0 0 0 3px var(--danger-soft);
}

.vpn-hero-content {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

/* Two explicit switches at the top: the tunnel and the browsers. */
.vpn-toggles {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.vpn-meta {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
  gap: 8px;
}

.vpn-meta div {
  min-width: 0;
  padding: 9px 10px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--muted);
}

.vpn-meta span {
  display: block;
  color: var(--text-3);
  font-size: 11.5px;
  font-weight: 500;
  letter-spacing: 0;
  text-transform: none;
}

.vpn-meta strong {
  display: block;
  overflow: hidden;
  color: var(--text);
  font-family: var(--mono);
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.vpn-page-link {
  margin-top: 2px;
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--accent-text);
  font: inherit;
  font-size: 11px;
  text-align: left;
  cursor: pointer;
}

.vpn-page-link:hover {
  text-decoration: underline;
}

.vpn-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.vpn-hint-card {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 16px;
  border-color: color-mix(in oklab, var(--warn) 45%, transparent);
  background: var(--warn-soft);
  color: var(--warn);
}

/* The Telegram prompt lives inside the subscription card, right above the list. */
.vpn-telegram-prompt {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 14px;
  border: 1px solid color-mix(in oklab, var(--warn) 45%, transparent);
  border-radius: 12px;
  background: var(--warn-soft);
}

/* The system proxy guard is active: informational, not a warning. */
.vpn-guard-card {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 16px;
  border-color: color-mix(in oklab, var(--accent) 35%, transparent);
  background: var(--accent-soft);
}

/* Telegram is already tunnelling: the banner turns from a task into a status. */
.vpn-ok-card {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 16px;
  border-color: color-mix(in oklab, var(--ok) 45%, transparent);
  background: var(--ok-soft);
  color: var(--ok);
}

.vpn-hint-icon {
  display: grid;
  place-items: center;
  width: 34px;
  height: 34px;
  flex: 0 0 auto;
  border-radius: 10px;
  background: var(--accent-soft);
  color: var(--accent-text);
}

.vpn-hint-text {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.vpn-hint-text strong {
  font-size: 13.5px;
}

.vpn-hint-text span {
  color: var(--text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.vpn-hint-text code {
  font-family: var(--mono);
  font-size: 11.5px;
}

.vpn-hint {
  color: var(--text-3);
  font-size: 12px;
  line-height: 1.5;
}

.vpn-hint code,
.vpn-hint strong {
  color: var(--text-2);
  font-family: var(--mono);
  font-size: 11.5px;
}

.vpn-error {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 12px;
  border: 1px solid color-mix(in oklab, var(--danger) 40%, transparent);
  border-radius: 9px;
  background: var(--danger-soft);
  color: var(--danger);
  font-size: 12px;
  line-height: 1.45;
}

.vpn-link-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.vpn-link {
  flex: 1 1 auto;
  min-width: 0;
  overflow-x: auto;
  padding: 9px 11px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--muted);
  color: var(--text-2);
  font-family: var(--mono);
  font-size: 12px;
  white-space: nowrap;
}

.vpn-fields {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 12px;
}

.vpn-fields .field-wide {
  grid-column: 1 / -1;
}

.vpn-input {
  width: 100%;
  height: 36px;
  padding: 0 11px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--secondary);
  color: var(--text);
  font: inherit;
  font-size: 13px;
}

.vpn-input:focus-visible {
  outline: none;
  border-color: var(--primary);
  box-shadow: var(--focus-ring);
}

.vpn-mono {
  font-family: var(--mono);
  font-size: 12px;
}

/* Subscriptions, buttons and the location list live in one card now. */
.vpn-locations {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding-top: 10px;
  border-top: 1px solid var(--border);
}

.vpn-locations-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
}

.vpn-locations-head strong {
  font-size: 13.5px;
  font-weight: 600;
}

.vpn-locations-head span {
  color: var(--text-3);
  font-size: 11.5px;
}

.vpn-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 380px;
  overflow-y: auto;
  /* Same gap between the rows and their scrollbar as the strategy list. */
  padding-right: 4px;
}

.vpn-list-head {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto auto;
  gap: 12px;
  padding: 0 12px 4px;
  color: var(--text-3);
  font-size: 11px;
}

.vpn-list-head span:last-child {
  min-width: 56px;
  text-align: right;
}

.vpn-list > * {
  flex: 0 0 auto;
}

.vpn-site-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.vpn-groups {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}

.vpn-groups-label {
  color: var(--text-3);
  font-size: 11.5px;
}

.vpn-bundle-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

/* Sits on the same baseline as the bundle picker, pinned to the right edge. */
.vpn-bases-btn {
  margin-left: auto;
}

.vpn-select {
  flex: 0 1 260px;
  cursor: pointer;
}

.vpn-suggest {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 12px;
  border: 1px solid color-mix(in oklab, var(--warn) 45%, transparent);
  border-radius: 9px;
  background: var(--warn-soft);
  color: var(--warn);
}

.vpn-suggest-head {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.vpn-suggest-head strong {
  font-size: 12px;
}

.vpn-suggest-head span {
  color: var(--text-2);
  font-size: 11.5px;
}

.vpn-suggest-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-height: 180px;
  overflow-y: auto;
}

.vpn-suggest-list > * {
  flex: 0 0 auto;
}

.vpn-suggest-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 2px 4px;
  color: var(--text);
  font-size: 12px;
  cursor: pointer;
}

.vpn-suggest-row code {
  overflow: hidden;
  font-family: var(--mono);
  font-size: 11.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.vpn-suggest-hits {
  margin-left: auto;
  color: var(--text-3);
  font-family: var(--mono);
  font-size: 11px;
}

.vpn-details {
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--secondary);
}

.vpn-details > summary {
  padding: 9px 12px;
  color: var(--text-2);
  font-size: 12.5px;
  cursor: pointer;
  user-select: none;
}

.vpn-details > summary::marker {
  color: var(--text-3);
}

.vpn-details .vpn-sites {
  padding: 0 12px 12px;
}

.vpn-group {
  padding: 4px 10px;
  border: 1px dashed var(--border);
  border-radius: 999px;
  background: transparent;
  color: var(--text-2);
  font: inherit;
  font-size: 11px;
  cursor: pointer;
}

.vpn-group:hover:not(:disabled) {
  border-color: var(--primary);
  color: var(--text);
}

.vpn-group:disabled {
  cursor: default;
  opacity: 0.5;
}

.vpn-site-row input {
  flex: 1 1 auto;
}

.vpn-sites {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.vpn-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 6px 4px 10px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--accent-soft);
  color: var(--text);
  font-family: var(--mono);
  font-size: 12px;
}

.vpn-chip-remove {
  display: grid;
  place-items: center;
  width: 22px;
  height: 22px;
  border: 0;
  border-radius: 50%;
  background: transparent;
  color: var(--text-2);
  font-size: 15px;
  line-height: 1;
  cursor: pointer;
}

.vpn-chip-remove:hover:not(:disabled) {
  background: var(--danger-soft);
  color: var(--danger);
}

.vpn-chip-remove:disabled {
  cursor: default;
  opacity: 0.5;
}

.vpn-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto auto;
  gap: 12px;
  align-items: center;
  width: 100%;
  min-height: 38px;
  padding: 8px 12px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--secondary);
  color: var(--text);
  font: inherit;
  font-size: 13px;
  text-align: left;
  cursor: pointer;
}

.vpn-row:hover:not(:disabled) {
  border-color: var(--primary);
}

.vpn-row.selected {
  border-color: var(--primary);
  background: var(--accent-soft);
}

.vpn-row:disabled {
  cursor: default;
  opacity: 0.7;
}

.vpn-row-name {
  display: flex;
  align-items: center;
  gap: 6px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.vpn-row-check {
  flex: 0 0 auto;
  color: var(--accent-text);
}

.vpn-row-scheme {
  color: var(--text-3);
  font-family: var(--mono);
  font-size: 11.5px;
}

.vpn-row-ping {
  min-width: 56px;
  color: var(--text-2);
  font-family: var(--mono);
  font-size: 12px;
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.vpn-log-head > div {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.vpn-log-head {
  display: flex;
  flex-direction: row;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

/* Advanced block: ports, core and logs stay out of the way until needed. */
.vpn-advanced > summary {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding: 14px 16px;
  color: var(--text);
  font-size: var(--fs-title);
  font-weight: 600;
  list-style: none;
  cursor: pointer;
  user-select: none;
}

.vpn-advanced > summary::-webkit-details-marker {
  display: none;
}

.vpn-advanced > summary::before {
  content: "▸";
  color: var(--text-3);
  font-weight: 400;
}

.vpn-advanced[open] > summary::before {
  content: "▾";
}

.vpn-advanced > summary .muted {
  font-size: var(--fs-caption);
  font-weight: 400;
}

.vpn-advanced-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 0 16px 16px;
}

.vpn-log {
  max-height: 240px;
  margin: 0;
  overflow: auto;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--secondary);
  color: var(--text-2);
  font-family: var(--mono);
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
}

@media (max-width: 860px) {
  .vpn-meta,
  .vpn-fields {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
