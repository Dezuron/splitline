<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import {
  AlertTriangle,
  Copy,
  Download,
  ExternalLink,
  RefreshCw,
  RotateCw,
  Send,
  X,
} from "lucide-vue-next";
import Card from "../components/ui/card/Card.vue";
import CardContent from "../components/ui/card/CardContent.vue";
import CardDescription from "../components/ui/card/CardDescription.vue";
import CardHeader from "../components/ui/card/CardHeader.vue";
import CardTitle from "../components/ui/card/CardTitle.vue";
import Button from "../components/ui/button/Button.vue";
import Switch from "../components/ui/switch/Switch.vue";
import { formatBytes } from "../lib/labels";
import {
  displayHost,
  isPublicBinding,
  MAX_CF_DOMAINS,
  MAX_DC_IPS,
  MAX_WORKER_DOMAINS,
  normalizeTgwsEngine,
  TGWS_BUF_MAX_KB,
  TGWS_BUF_MIN_KB,
  TGWS_ENGINES,
  TGWS_POOL_MAX,
  TGWS_POOL_MIN,
  tgwsEndpoint,
  tgwsEngineLabel,
  tgwsLimitsError,
  tgwsTone,
  tgwsVersionLabel,
  type TgwsEngine,
} from "../lib/tgws";
import {
  applyTgwsUpdate,
  checkTgwsUpdate,
  config,
  copyTgwsLink,
  dismissTgwsUpdate,
  diagnoseTgws,
  openReleasePage,
  openTgwsInTelegram,
  openTgwsLog,
  openXrayInTelegram,
  refreshTgws,
  regenerateTgwsSecret,
  restartTgws,
  setTgwsBalance,
  setTgwsDcIps,
  setTgwsDomains,
  setTgwsEngine,
  setTgwsFakeTlsDomain,
  setTgwsFlags,
  setTgwsHost,
  setTgwsLimits,
  setTgwsPort,
  setTgwsSecret,
  setTgwsWorkerDomains,
  switchTgwsPort,
  tgwsBusy,
  tgwsChecking,
  tgwsHasUpdate,
  tgwsLink,
  tgwsLogs,
  tgwsDiagnosis,
  tgwsDiagnosing,
  tgwsProgress,
  tgwsRunning,
  tgwsActiveConnections,
  tgwsActivity,
  tgwsLastActivityLabel,
  tgwsStatus,
  tgwsStatusText,
  tgwsUpdate,
  toggleTgws,
  xrayStatus,
} from "../store";
import type { TgwsFlags } from "../types";

const hostDraft = ref("");
const portDraft = ref(1555);
const secretDraft = ref("");
const engineError = ref("");
const domainDraft = ref("");
const domainError = ref("");
const domainsBusy = ref(false);
const workerDraft = ref("");
const workerError = ref("");
const cfFlagsError = ref("");
const expertFlagsError = ref("");
const dcDraft = ref("");
const dcError = ref("");
const dcBusy = ref(false);
const bufDraft = ref("");
const poolDraft = ref("");
const limitsError = ref("");
const secretBusy = ref(false);
const fakeTlsDraft = ref("");
const fakeTlsError = ref("");
const logsOpen = ref(false);
const logBox = ref<HTMLElement | null>(null);
const router = useRouter();

/** Flowseal's own instructions, opened in the browser via `openReleasePage`. */
const CF_PROXY_DOC = "https://github.com/Flowseal/tg-ws-proxy/blob/main/docs/RU/CfProxy.md";
const CF_WORKER_DOC = "https://github.com/Flowseal/tg-ws-proxy/blob/main/docs/RU/CfWorker.md";

/** Hides the check result: the card is only shown on demand. */
function closeDiagnosis() {
  tgwsDiagnosis.value = null;
}

function openVpnForTelegram() {
  if (config.value?.xray.subscription_url) {
    void openXrayInTelegram();
  } else {
    void router.push("/vpn");
  }
}

const tone = computed(() => tgwsTone(tgwsStatus.value?.state));
const endpointLabel = computed(() =>
  tgwsEndpoint(config.value?.tgws.host, config.value?.tgws.port),
);
const versionLabel = computed(() =>
  tgwsVersionLabel(
    config.value?.tgws.installed_version || tgwsStatus.value?.installed_version,
    tgwsStatus.value?.binary_present,
  ),
);
const transitioning = computed(
  () => tgwsStatus.value?.state === "starting" || tgwsStatus.value?.state === "stopping",
);
const publicBinding = computed(() => isPublicBinding(config.value?.tgws.host));
const logText = computed(() => tgwsLogs.value.join("\n"));

/** Engine the backend actually runs; `python` when an older build does not say. */
const engine = computed<TgwsEngine>(() =>
  normalizeTgwsEngine(tgwsStatus.value?.engine ?? config.value?.tgws.engine),
);

/** R8: the new engine keeps failing, so the page offers the old one. */
const needsFallback = computed(() => !!tgwsStatus.value?.fallback_recommended);

/**
 * Settings only the new engine understands. A build that does not report them
 * leaves the block read-only instead of silently overwriting the config file.
 */
const engineSettingsKnown = computed(() => Array.isArray(config.value?.tgws.cf_domains));
const fakeTlsKnown = computed(() => typeof config.value?.tgws.fake_tls_domain === "string");
const cfDomains = computed(() => config.value?.tgws.cf_domains ?? []);
const cfBalance = computed(() => config.value?.tgws.cf_balance ?? false);
const canAddDomain = computed(
  () => engineSettingsKnown.value && cfDomains.value.length < MAX_CF_DOMAINS,
);
const fakeTlsOn = computed(() => fakeTlsDraft.value.trim().length > 0);

/** PROXY-1 fields; an older build that does not report them stays read-only. */
const workerDomainsKnown = computed(() => Array.isArray(config.value?.tgws.cf_worker_domains));
const workerDomains = computed(() => config.value?.tgws.cf_worker_domains ?? []);
const canAddWorker = computed(
  () => workerDomainsKnown.value && workerDomains.value.length < MAX_WORKER_DOMAINS,
);
const flagsKnown = computed(() => {
  const tgws = config.value?.tgws;
  // All six travel together: sending a partial set would reset the missing
  // flags to their defaults.
  return (
    typeof tgws?.cfproxy === "boolean" &&
    typeof tgws?.h2 === "boolean" &&
    typeof tgws?.verbose === "boolean" &&
    typeof tgws?.no_secure === "boolean" &&
    typeof tgws?.force_test_dc === "boolean" &&
    typeof tgws?.proxy_protocol === "boolean"
  );
});
const cfProxy = computed(() => config.value?.tgws.cfproxy ?? true);
const h2Media = computed(() => config.value?.tgws.h2 ?? true);
const verbose = computed(() => config.value?.tgws.verbose ?? false);
const noSecure = computed(() => config.value?.tgws.no_secure ?? false);
const forceTestDc = computed(() => config.value?.tgws.force_test_dc ?? false);
const proxyProtocol = computed(() => config.value?.tgws.proxy_protocol ?? false);
const dcIpsKnown = computed(() => Array.isArray(config.value?.tgws.dc_ip));
const dcIps = computed(() => config.value?.tgws.dc_ip ?? []);
const canAddDc = computed(() => dcIpsKnown.value && dcIps.value.length < MAX_DC_IPS);
const limitsKnown = computed(
  () =>
    typeof config.value?.tgws.buf_kb === "number" &&
    typeof config.value?.tgws.pool_size === "number",
);

/** Last saved limits: polling refreshes config, but must not erase a bad draft. */
let savedBufKb: number | null = null;
let savedPoolSize: number | null = null;

/** Switches the engine; the backend installs the binary and restarts the proxy. */
async function chooseEngine(next: TgwsEngine) {
  if (next === engine.value || tgwsBusy.value) return;
  engineError.value = "";
  try {
    await setTgwsEngine(next);
  } catch (error) {
    engineError.value = String(error);
  }
}

/**
 * Saves the whole list, because the backend keeps it as one ordered setting.
 * A failure keeps the typed text: the toast and the line under the list say why.
 */
async function saveDomains(next: string[]): Promise<boolean> {
  if (tgwsBusy.value) return false;
  domainError.value = "";
  domainsBusy.value = true;
  try {
    await setTgwsDomains(next);
    return true;
  } catch (error) {
    domainError.value = String(error);
    return false;
  } finally {
    domainsBusy.value = false;
  }
}

async function addDomain() {
  const value = domainDraft.value.trim();
  if (!value || !canAddDomain.value) return;
  if (await saveDomains([...cfDomains.value, value])) domainDraft.value = "";
}

async function removeDomain(domain: string) {
  await saveDomains(cfDomains.value.filter((item) => item !== domain));
}

async function toggleBalance(enabled: boolean) {
  if (!engineSettingsKnown.value) return;
  domainError.value = "";
  try {
    await setTgwsBalance(enabled);
  } catch (error) {
    domainError.value = String(error);
  }
}

/**
 * The expert flags are one backend setting: the command replaces all of them,
 * so a toggle sends the current values plus the one that changed.
 */
function currentFlags(): TgwsFlags {
  return {
    cfproxy: cfProxy.value,
    h2: h2Media.value,
    verbose: verbose.value,
    no_secure: noSecure.value,
    force_test_dc: forceTestDc.value,
    proxy_protocol: proxyProtocol.value,
  };
}

/** Saves one flag; returns why it failed, or an empty string. */
async function applyFlags(patch: Partial<TgwsFlags>): Promise<string> {
  if (!flagsKnown.value || tgwsBusy.value) return "";
  try {
    await setTgwsFlags({ ...currentFlags(), ...patch });
    return "";
  } catch (error) {
    return String(error);
  }
}

async function toggleCfFlag(patch: Partial<TgwsFlags>) {
  cfFlagsError.value = await applyFlags(patch);
}

async function toggleExpertFlag(patch: Partial<TgwsFlags>) {
  expertFlagsError.value = await applyFlags(patch);
}

/** Saves the DC redirect list; the engine restarts when it was running. */
async function saveDcIps(next: string[]): Promise<boolean> {
  if (tgwsBusy.value) return false;
  dcError.value = "";
  dcBusy.value = true;
  try {
    await setTgwsDcIps(next);
    return true;
  } catch (error) {
    dcError.value = String(error);
    return false;
  } finally {
    dcBusy.value = false;
  }
}

async function addDcIp() {
  const value = dcDraft.value.trim();
  if (!value || !canAddDc.value) return;
  if (await saveDcIps([...dcIps.value, value])) dcDraft.value = "";
}

async function removeDcIp(entry: string) {
  await saveDcIps(dcIps.value.filter((item) => item !== entry));
}

/**
 * Saves both numbers at once. A rejected value stays in its field with the
 * reason underneath, so the user can fix one digit instead of retyping.
 */
async function commitLimits() {
  if (!limitsKnown.value || tgwsBusy.value) return;
  const problem = tgwsLimitsError(bufDraft.value, poolDraft.value);
  limitsError.value = problem;
  if (problem) return;
  const buf = Number(bufDraft.value.trim());
  const pool = Number(poolDraft.value.trim());
  if (buf === config.value?.tgws.buf_kb && pool === config.value?.tgws.pool_size) return;
  try {
    await setTgwsLimits({ buf_kb: buf, pool_size: pool });
  } catch (error) {
    limitsError.value = String(error);
  }
}

/**
 * The key button spins only for the key itself: every other settings save also
 * sets `tgwsBusy` while the backend restarts the proxy.
 */
async function regenerateKey() {
  if (tgwsBusy.value) return;
  secretBusy.value = true;
  try {
    await regenerateTgwsSecret();
  } finally {
    secretBusy.value = false;
  }
}

/** Saves the Worker list; the backend restarts a running proxy. */
async function saveWorkerDomains(next: string[]): Promise<boolean> {
  if (tgwsBusy.value) return false;
  workerError.value = "";
  domainsBusy.value = true;
  try {
    await setTgwsWorkerDomains(next);
    return true;
  } catch (error) {
    workerError.value = String(error);
    return false;
  } finally {
    domainsBusy.value = false;
  }
}

async function addWorkerDomain() {
  const value = workerDraft.value.trim();
  if (!value || !canAddWorker.value) return;
  if (await saveWorkerDomains([...workerDomains.value, value])) workerDraft.value = "";
}

async function removeWorkerDomain(domain: string) {
  await saveWorkerDomains(workerDomains.value.filter((item) => item !== domain));
}

/** Clears the field and saves right away: an empty domain turns FakeTLS off. */
async function clearFakeTls() {
  fakeTlsDraft.value = "";
  await commitFakeTls();
}

/** FakeTLS changes the link format, so the backend restarts the proxy here. */
async function commitFakeTls() {
  const saved = config.value?.tgws.fake_tls_domain ?? "";
  const value = fakeTlsDraft.value.trim();
  if (value === saved.trim()) return;
  fakeTlsError.value = "";
  try {
    fakeTlsDraft.value = await setTgwsFakeTlsDomain(value);
  } catch (error) {
    fakeTlsError.value = String(error);
  }
}

/** Telegram keeps one proxy at a time: an active VPN tunnel wins over MTProto. */
const telegramOnVpn = computed(
  () =>
    xrayStatus.value?.state === "running" &&
    (xrayStatus.value?.client_connections ?? 0) > 0 &&
    tgwsActivity.value === "idle",
);
/** The proxy is on, but Telegram has not connected to it yet. */
const telegramNeedsProxy = computed(() => tgwsRunning.value && tgwsActivity.value === "idle");

/** Starts the proxy if needed and hands Telegram the `tg://` link. */
async function switchTelegramToProxy() {
  if (!tgwsRunning.value) {
    await toggleTgws();
  }
  await openTgwsInTelegram();
}

watch(
  () => config.value?.tgws,
  (tgws) => {
    if (!tgws) return;
    hostDraft.value = tgws.host;
    portDraft.value = tgws.port;
    secretDraft.value = tgws.secret;
    if (typeof tgws.fake_tls_domain === "string") fakeTlsDraft.value = tgws.fake_tls_domain;
  },
  { immediate: true, deep: true },
);

/**
 * Limits are edited in place, so the drafts follow the saved values only when
 * those actually change. A background config refresh must not wipe a number
 * the user is about to correct.
 */
watch(
  () => [config.value?.tgws.buf_kb, config.value?.tgws.pool_size] as const,
  ([buf, pool]) => {
    if (typeof buf === "number" && buf !== savedBufKb) {
      savedBufKb = buf;
      bufDraft.value = String(buf);
    }
    if (typeof pool === "number" && pool !== savedPoolSize) {
      savedPoolSize = pool;
      poolDraft.value = String(pool);
    }
  },
  { immediate: true },
);

watch(logText, async () => {
  if (!logsOpen.value) return;
  await nextTick();
  logBox.value?.scrollTo({ top: logBox.value.scrollHeight });
});

onMounted(async () => {
  await refreshTgws();
  await checkTgwsUpdate(true);
});

async function commitHost() {
  const value = hostDraft.value.trim();
  if (!value || value === config.value?.tgws.host) return;
  try {
    hostDraft.value = await setTgwsHost(value);
  } catch {
    hostDraft.value = config.value?.tgws.host ?? "127.0.0.1";
  }
}

async function commitPort() {
  const value = Number(portDraft.value);
  if (!Number.isFinite(value) || value === config.value?.tgws.port) return;
  try {
    portDraft.value = await setTgwsPort(value);
  } catch {
    portDraft.value = config.value?.tgws.port ?? 1555;
  }
}

async function commitSecret() {
  const value = secretDraft.value.trim();
  if (!value || value === config.value?.tgws.secret) return;
  try {
    secretDraft.value = await setTgwsSecret(value);
  } catch {
    secretDraft.value = config.value?.tgws.secret ?? "";
  }
}
</script>

<template>
  <div class="page-stack tgws-page">
    <Card class="tgws-card tgws-hero">
      <CardHeader class="tgws-hero-head">
        <div class="tgws-hero-icon"><Send :size="21" /></div>
        <div class="tgws-hero-text">
          <span class="eyebrow">Telegram</span>
          <CardTitle>WS-прокси</CardTitle>
          <CardDescription>{{ tgwsStatusText }}</CardDescription>
        </div>
        <div class="tgws-hero-controls">
          <span class="tgws-dot" :class="`is-${tone}`" aria-hidden="true"></span>
          <Switch
            :model-value="tgwsRunning"
            :disabled="tgwsBusy || transitioning"
            aria-label="Включить или выключить Telegram-прокси"
            @update:model-value="toggleTgws"
          />
        </div>
      </CardHeader>
      <CardContent class="tgws-hero-content">
        <div class="tgws-meta">
          <div>
            <span>Адрес</span>
            <strong>{{ endpointLabel }}</strong>
          </div>
          <div>
            <span>Версия</span>
            <strong>{{ versionLabel }}</strong>
          </div>
          <div>
            <span>PID</span>
            <strong>{{ tgwsStatus?.pid ?? "—" }}</strong>
          </div>
          <div>
            <span>Подключений</span>
            <strong>{{ tgwsActiveConnections ?? "—" }}</strong>
          </div>
          <div>
            <span>Активность</span>
            <strong>{{ tgwsLastActivityLabel }}</strong>
          </div>
        </div>

        <div class="tgws-actions">
          <Button
            v-if="tgwsRunning"
            size="sm"
            :disabled="!tgwsLink || transitioning"
            @click="openTgwsInTelegram"
          >
            <ExternalLink :size="14" />
            Открыть в Telegram
          </Button>
          <Button
            v-if="tgwsRunning"
            variant="outline"
            size="sm"
            :disabled="!tgwsLink"
            @click="copyTgwsLink"
          >
            <Copy :size="14" />
            Скопировать ссылку
          </Button>
          <Button
            variant="secondary"
            size="sm"
            :disabled="tgwsDiagnosing || transitioning"
            @click="diagnoseTgws"
          >
            {{ tgwsDiagnosing ? "Проверка…" : "Проверить прокси" }}
          </Button>
          <Button
            variant="outline"
            size="sm"
            :disabled="tgwsBusy || transitioning || !tgwsRunning"
            @click="restartTgws"
          >
            <RotateCw :size="14" />
            Перезапустить
          </Button>
          <Button
            variant="ghost"
            size="sm"
            :disabled="tgwsChecking"
            @click="checkTgwsUpdate(false, true)"
          >
            <RefreshCw :size="14" :class="{ spinning: tgwsChecking }" />
            {{ tgwsChecking ? "Проверка…" : "Проверить обновление" }}
          </Button>
        </div>

        <p v-if="tgwsProgress" class="tgws-progress">{{ tgwsProgress }}</p>
        <div v-if="tgwsStatus?.last_error" class="tgws-error">
          <AlertTriangle :size="14" />
          <div class="tgws-error-body">
            <span>{{ tgwsStatus.last_error }}</span>
            <Button
              v-if="tgwsStatus?.port_conflict"
              variant="outline"
              size="sm"
              :disabled="tgwsBusy"
              @click="switchTgwsPort"
            >
              Занять свободный порт
            </Button>
          </div>
        </div>

        <div v-if="needsFallback" class="tgws-prompt is-danger">
          <div class="tgws-prompt-icon"><AlertTriangle :size="18" /></div>
          <div class="tgws-prompt-text">
            <strong>Новый движок не запускается</strong>
            <span>
              Вернитесь на старый — ключ и ссылка останутся прежними, добавлять прокси в Telegram
              заново не нужно.
            </span>
          </div>
          <Button size="sm" :disabled="tgwsBusy || transitioning" @click="chooseEngine('python')">
            Вернуться на старый
          </Button>
        </div>

        <div v-if="telegramOnVpn" class="tgws-prompt">
          <div class="tgws-prompt-icon"><Send :size="18" /></div>
          <div class="tgws-prompt-text">
            <strong>Telegram сидит на VPN, а не на MTProto-прокси</strong>
            <span>
              В Telegram может быть активен только один прокси, поэтому трафик идёт через VPN.
              Переключить Telegram на прокси?
            </span>
          </div>
          <Button size="sm" :disabled="tgwsBusy || transitioning" @click="switchTelegramToProxy">
            Переключить на прокси
          </Button>
        </div>

        <div v-else-if="telegramNeedsProxy" class="tgws-prompt">
          <div class="tgws-prompt-icon"><Send :size="18" /></div>
          <div class="tgws-prompt-text">
            <strong>Telegram ещё не использует прокси</strong>
            <span>
              В Telegram выберите «Использовать собственный прокси» и добавьте ссылку — проще всего
              кнопкой ниже: ключ и порт уже верные, менять ничего не нужно.
            </span>
          </div>
          <Button size="sm" :disabled="!tgwsLink" @click="openTgwsInTelegram">
            Подключить Telegram
          </Button>
        </div>
      </CardContent>
    </Card>

    <Card v-if="tgwsDiagnosis" class="tgws-card tgws-diag">
      <CardHeader>
        <div class="tgws-diag-head">
          <div>
            <CardTitle>Проверка прокси</CardTitle>
            <CardDescription>Результат последней проверки</CardDescription>
          </div>
          <Button
            variant="ghost"
            size="icon"
            title="Закрыть"
            aria-label="Закрыть результат проверки"
            @click="closeDiagnosis"
          >
            <X :size="15" />
          </Button>
        </div>
      </CardHeader>
      <CardContent class="tgws-diag-body">
        <p class="diag-note">
          Движок: <strong>{{ tgwsEngineLabel(tgwsDiagnosis.engine) }}</strong>
        </p>
        <div class="diag-list">
          <div class="diag-row" :class="{ bad: !tgwsDiagnosis.binary }">
            <span aria-hidden="true">{{ tgwsDiagnosis.binary ? "✓" : "✗" }}</span>
            Прокси установлен
          </div>
          <div class="diag-row" :class="{ bad: !tgwsDiagnosis.process_running }">
            <span aria-hidden="true">{{ tgwsDiagnosis.process_running ? "✓" : "✗" }}</span>
            Процесс запущен
          </div>
          <div class="diag-row" :class="{ bad: !tgwsDiagnosis.port_open }">
            <span aria-hidden="true">{{ tgwsDiagnosis.port_open ? "✓" : "✗" }}</span>
            Порт слушается
          </div>
          <div
            v-if="tgwsDiagnosis.listener"
            class="diag-row"
            :class="{
              bad: tgwsDiagnosis.listener.ran && !tgwsDiagnosis.listener.ok,
              'is-neutral': !tgwsDiagnosis.listener.ran,
            }"
          >
            <span aria-hidden="true">
              {{ tgwsDiagnosis.listener.ran ? (tgwsDiagnosis.listener.ok ? "✓" : "✗") : "–" }}
            </span>
            Проверка слушателя
          </div>
          <div class="diag-row" :class="{ bad: tgwsDiagnosis.log_errors > 0 }">
            <span aria-hidden="true">{{ tgwsDiagnosis.log_errors > 0 ? "✗" : "✓" }}</span>
            Связь с серверами прокси
            <span v-if="tgwsDiagnosis.log_errors > 0" class="muted">
              · ошибок: {{ tgwsDiagnosis.log_errors }}
            </span>
          </div>
          <div class="diag-row" :class="{ bad: tgwsDiagnosis.clients === 0 }">
            <span aria-hidden="true">{{ tgwsDiagnosis.clients > 0 ? "✓" : "✗" }}</span>
            Telegram подключён
            <span v-if="tgwsDiagnosis.clients > 0" class="muted">
              · соединений: {{ tgwsDiagnosis.clients }}
            </span>
          </div>
        </div>

        <p class="diag-hint" :class="`is-${tgwsDiagnosis.kind}`">{{ tgwsDiagnosis.hint }}</p>

        <p v-if="tgwsDiagnosis.last_error_line" class="diag-note">
          Последняя ошибка в логе:
          <span class="tgws-mono">{{ tgwsDiagnosis.last_error_line }}</span>
        </p>

        <template v-if="tgwsDiagnosis.listener">
          <p class="diag-note">{{ tgwsDiagnosis.listener.summary }}</p>
          <p
            v-if="tgwsDiagnosis.listener.detail && !tgwsDiagnosis.listener.ok"
            class="diag-note tgws-mono"
          >
            {{ tgwsDiagnosis.listener.detail }}
          </p>
        </template>

        <div class="tgws-actions">
          <Button
            v-if="tgwsDiagnosis.kind === 'port'"
            size="sm"
            :disabled="tgwsBusy"
            @click="switchTgwsPort"
          >
            Занять свободный порт
          </Button>
          <Button
            v-if="tgwsDiagnosis.kind === 'stopped'"
            variant="secondary"
            size="sm"
            :disabled="tgwsBusy"
            @click="restartTgws"
          >
            Перезапустить
          </Button>
          <Button
            v-if="['binary', 'blocked'].includes(tgwsDiagnosis.kind)"
            variant="secondary"
            size="sm"
            :disabled="tgwsBusy"
            @click="applyTgwsUpdate"
          >
            Обновить прокси
          </Button>
          <Button
            v-if="tgwsDiagnosis.kind === 'not_connected'"
            variant="secondary"
            size="sm"
            :disabled="!tgwsLink"
            @click="openTgwsInTelegram"
          >
            Подключить Telegram
          </Button>
          <Button variant="secondary" size="sm" @click="openTgwsLog">Открыть лог</Button>
          <Button variant="ghost" size="sm" @click="openVpnForTelegram">VPN вместо прокси</Button>
        </div>
      </CardContent>
    </Card>

    <Card v-if="tgwsHasUpdate && tgwsUpdate?.latest" class="tgws-card tgws-update">
      <CardContent class="tgws-update-content">
        <div class="tgws-update-icon"><Download :size="18" /></div>
        <div class="tgws-update-text">
          <strong>Доступно обновление прокси Flowseal {{ tgwsUpdate.latest }}</strong>
          <span>
            Установлено: {{ tgwsUpdate.installed || "неизвестно" }}
            <template v-if="tgwsUpdate.asset_size">
              · {{ formatBytes(tgwsUpdate.asset_size) }}
            </template>
          </span>
          <button
            v-if="tgwsUpdate.release_url"
            type="button"
            class="tgws-update-link"
            @click="openReleasePage(tgwsUpdate.release_url as string)"
          >
            Что нового в Flowseal/tg-ws-proxy
          </button>
        </div>
        <div class="tgws-update-actions">
          <Button variant="ghost" size="sm" :disabled="tgwsBusy" @click="dismissTgwsUpdate">
            Позже
          </Button>
          <Button size="sm" :disabled="tgwsBusy" @click="applyTgwsUpdate">
            <Download :size="14" />
            Обновить
          </Button>
        </div>
      </CardContent>
    </Card>

    <Card class="tgws-card">
      <CardHeader>
        <CardTitle>Подключение</CardTitle>
        <CardDescription>
          Вставьте ссылку в Telegram → Настройки → Продвинутые настройки → Тип соединения, либо
          откройте её кнопкой справа.
        </CardDescription>
      </CardHeader>
      <CardContent class="tgws-connect">
        <div class="tgws-link-row">
          <code class="tgws-link" :title="tgwsLink">{{
            tgwsLink || "Ключ ещё не сгенерирован"
          }}</code>
          <Button
            variant="outline"
            size="icon"
            title="Копировать ссылку"
            aria-label="Копировать ссылку"
            :disabled="!tgwsLink"
            @click="copyTgwsLink"
          >
            <Copy :size="15" />
          </Button>
          <Button
            variant="outline"
            size="icon"
            title="Открыть в Telegram"
            aria-label="Открыть ссылку в Telegram"
            :disabled="!tgwsLink"
            @click="openTgwsInTelegram"
          >
            <ExternalLink :size="15" />
          </Button>
          <Button
            variant="outline"
            size="icon"
            title="Сменить ключ и ссылку"
            aria-label="Сменить ключ и ссылку"
            :disabled="tgwsBusy"
            @click="regenerateKey"
          >
            <RefreshCw :size="15" :class="{ spinning: secretBusy }" />
          </Button>
        </div>
        <p class="tgws-hint">
          Ключ живёт в <code>config.json</code> и не меняется между запусками. Смена ключа сразу
          перезапускает прокси, старые ссылки перестают работать.
        </p>
      </CardContent>
    </Card>

    <Card class="tgws-card">
      <details class="tgws-advanced">
        <summary>
          <span>Дополнительно</span>
          <span class="muted">Движок, Cloudflare, DC, лимиты и логи</span>
        </summary>
        <div class="tgws-advanced-body">
          <div class="tgws-section">
            <div class="tgws-section-head">
              <span class="field-label">Движок прокси</span>
              <span class="muted">{{
                engine === "rs" ? "Новый, лёгкий — beta" : "Старый, проверенный"
              }}</span>
            </div>
            <div class="tgws-choice" role="radiogroup" aria-label="Движок прокси">
              <button
                v-for="option in TGWS_ENGINES"
                :key="option.value"
                type="button"
                role="radio"
                class="tgws-choice-option"
                :class="{ 'is-active': engine === option.value }"
                :aria-checked="engine === option.value"
                :disabled="tgwsBusy || transitioning"
                @click="chooseEngine(option.value)"
              >
                <strong>{{ option.label }}</strong>
                <span>{{ option.note }}</span>
              </button>
            </div>
            <p class="tgws-warning">
              <AlertTriangle :size="14" />
              <span>
                Переключение перезапустит прокси на несколько секунд. Ключ и ссылка не меняются,
                поэтому прокси в Telegram добавлять заново не придётся.
              </span>
            </p>
            <p v-if="engine === 'rs' && !tgwsStatus?.rust_binary_present" class="tgws-hint">
              Новый движок скачается при переключении — один раз, несколько мегабайт.
            </p>
            <div v-if="engineError" class="tgws-error">
              <AlertTriangle :size="14" />
              <span>{{ engineError }}</span>
            </div>
          </div>

          <div class="tgws-section">
            <div class="tgws-section-head">
              <span class="field-label">Cloudflare</span>
              <span class="muted">домены, медиа, маскировка</span>
            </div>

            <div class="tgws-balance">
              <Switch
                :model-value="cfProxy"
                :disabled="!flagsKnown || tgwsBusy"
                aria-label="Проксировать через Cloudflare"
                @update:model-value="toggleCfFlag({ cfproxy: $event })"
              />
              <span>
                <strong>CF-прокси</strong>
                <span class="muted">
                  Обход блокировок через Cloudflare: свои домены или встроенный список.
                </span>
              </span>
            </div>

            <div class="tgws-balance">
              <Switch
                :model-value="h2Media"
                :disabled="!flagsKnown || tgwsBusy"
                aria-label="Мультиплексировать медиа через HTTP/2"
                @update:model-value="toggleCfFlag({ h2: $event })"
              />
              <span>
                <strong>H2-медиа</strong>
                <span class="muted">
                  Фото и видео идут одним общим соединением. Работает при включённом CF-прокси.
                </span>
              </span>
            </div>

            <p v-if="!flagsKnown" class="tgws-hint">
              Не удалось прочитать настройки Cloudflare — обновите приложение.
            </p>
            <p v-if="cfFlagsError" class="tgws-error">
              <AlertTriangle :size="14" />
              <span>{{ cfFlagsError }}</span>
            </p>

            <div class="tgws-sub">
              <div class="tgws-section-head">
                <span class="field-label">Свои домены</span>
                <span v-if="engineSettingsKnown" class="muted">
                  {{ cfDomains.length }} из {{ MAX_CF_DOMAINS }}
                </span>
              </div>
              <p v-if="!engineSettingsKnown" class="tgws-hint">
                Не удалось прочитать список доменов — обновите приложение.
              </p>
              <template v-else>
                <div v-if="cfDomains.length" class="tgws-chips">
                  <span v-for="domain in cfDomains" :key="domain" class="tgws-chip">
                    {{ domain }}
                    <button
                      type="button"
                      class="tgws-chip-remove"
                      :disabled="domainsBusy || tgwsBusy"
                      :aria-label="`Убрать ${domain}`"
                      @click="removeDomain(domain)"
                    >
                      ×
                    </button>
                  </span>
                </div>
                <div class="tgws-domain-row">
                  <input
                    v-model="domainDraft"
                    class="tgws-input tgws-mono"
                    spellcheck="false"
                    placeholder="example.com"
                    @keydown.enter="addDomain"
                  />
                  <Button
                    size="sm"
                    :disabled="domainsBusy || tgwsBusy || !canAddDomain || !domainDraft.trim()"
                    @click="addDomain"
                  >
                    Добавить
                  </Button>
                </div>
                <p v-if="domainError" class="tgws-error">
                  <AlertTriangle :size="14" />
                  <span>{{ domainError }}</span>
                </p>
                <p v-if="!cfDomains.length" class="tgws-hint">
                  Используется встроенный список Flowseal; он скачивается с GitHub при старте.
                  Инструкции Flowseal:
                  <button
                    type="button"
                    class="tgws-doc-link"
                    @click="openReleasePage(CF_PROXY_DOC)"
                  >
                    свой домен (CfProxy)
                  </button>
                  ·
                  <button
                    type="button"
                    class="tgws-doc-link"
                    @click="openReleasePage(CF_WORKER_DOC)"
                  >
                    Worker-домены (CfWorker)
                  </button>
                </p>
                <p v-else class="tgws-hint">
                  <template v-if="engine === 'python'">
                    Старый движок берёт только первый домен списка.
                  </template>
                  <template v-else>Новый движок пробует домены в этом порядке.</template>
                </p>
              </template>
            </div>

            <div class="tgws-sub">
              <div class="tgws-section-head">
                <span class="field-label">Worker-домены</span>
                <span v-if="workerDomainsKnown" class="muted">
                  {{ workerDomains.length }} из {{ MAX_WORKER_DOMAINS }}
                </span>
              </div>
              <p v-if="!workerDomainsKnown" class="tgws-hint">
                Не удалось прочитать список — обновите приложение.
              </p>
              <template v-else>
                <div v-if="workerDomains.length" class="tgws-chips">
                  <span v-for="domain in workerDomains" :key="domain" class="tgws-chip">
                    {{ domain }}
                    <button
                      type="button"
                      class="tgws-chip-remove"
                      :disabled="domainsBusy || tgwsBusy"
                      :aria-label="`Убрать ${domain}`"
                      @click="removeWorkerDomain(domain)"
                    >
                      ×
                    </button>
                  </span>
                </div>
                <div class="tgws-domain-row">
                  <input
                    v-model="workerDraft"
                    class="tgws-input tgws-mono"
                    spellcheck="false"
                    placeholder="my-worker.workers.dev"
                    @keydown.enter="addWorkerDomain"
                  />
                  <Button
                    size="sm"
                    :disabled="domainsBusy || tgwsBusy || !canAddWorker || !workerDraft.trim()"
                    @click="addWorkerDomain"
                  >
                    Добавить
                  </Button>
                </div>
                <p v-if="workerError" class="tgws-error">
                  <AlertTriangle :size="14" />
                  <span>{{ workerError }}</span>
                </p>
                <p v-else class="tgws-hint">
                  Запасной путь через Cloudflare Worker, если прокси не отвечает напрямую. Домены
                  пробуются по очереди.
                </p>
              </template>
            </div>

            <div class="tgws-balance">
              <Switch
                :model-value="cfBalance"
                :disabled="domainsBusy || !engineSettingsKnown || tgwsBusy"
                aria-label="Балансировать между доменами"
                @update:model-value="toggleBalance"
              />
              <span>
                <strong>Балансировать</strong>
                <span class="muted">Начинать с разных доменов, а не всегда с первого.</span>
              </span>
            </div>

            <div class="tgws-sub">
              <div class="tgws-section-head">
                <span class="field-label">FakeTLS-домен</span>
                <span class="muted">маскировка под обычный сайт</span>
              </div>
              <p v-if="!fakeTlsKnown" class="tgws-hint">
                Не удалось прочитать настройку — обновите приложение.
              </p>
              <template v-else>
                <div class="tgws-domain-row">
                  <input
                    v-model="fakeTlsDraft"
                    class="tgws-input tgws-mono"
                    spellcheck="false"
                    placeholder="www.cloudflare.com"
                    @change="commitFakeTls"
                    @keydown.enter="commitFakeTls"
                  />
                  <Button
                    v-if="fakeTlsOn"
                    variant="ghost"
                    size="sm"
                    :disabled="tgwsBusy"
                    @click="clearFakeTls"
                  >
                    Выключить
                  </Button>
                </div>
                <p v-if="fakeTlsError" class="tgws-error">
                  <AlertTriangle :size="14" />
                  <span>{{ fakeTlsError }}</span>
                </p>
                <p class="tgws-warning">
                  <AlertTriangle :size="14" />
                  <span>
                    Ссылка станет вида <code>ee…</code>. Старая ссылка в Telegram перестанет
                    подключаться — замените её на новую.
                  </span>
                </p>
              </template>
            </div>
          </div>

          <div class="tgws-section">
            <div class="tgws-section-head">
              <span class="field-label">DC → IP</span>
              <span v-if="dcIpsKnown" class="muted"> {{ dcIps.length }} из {{ MAX_DC_IPS }} </span>
            </div>
            <p v-if="!dcIpsKnown" class="tgws-hint">
              Не удалось прочитать список — обновите приложение.
            </p>
            <template v-else>
              <div v-if="dcIps.length" class="tgws-chips">
                <span v-for="entry in dcIps" :key="entry" class="tgws-chip">
                  {{ entry }}
                  <button
                    type="button"
                    class="tgws-chip-remove"
                    :disabled="dcBusy || tgwsBusy"
                    :aria-label="`Убрать ${entry}`"
                    @click="removeDcIp(entry)"
                  >
                    ×
                  </button>
                </span>
              </div>
              <div class="tgws-domain-row">
                <input
                  v-model="dcDraft"
                  class="tgws-input tgws-mono"
                  spellcheck="false"
                  placeholder="2:149.154.167.220"
                  @keydown.enter="addDcIp"
                />
                <Button
                  size="sm"
                  :disabled="dcBusy || tgwsBusy || !canAddDc || !dcDraft.trim()"
                  @click="addDcIp"
                >
                  Добавить
                </Button>
              </div>
              <p v-if="dcError" class="tgws-error">
                <AlertTriangle :size="14" />
                <span>{{ dcError }}</span>
              </p>
              <p class="tgws-hint">
                Формат — <code>DC:IP</code>. По умолчанию перенаправляются DC 2 и 4; пустой список
                означает адреса движка по умолчанию. Меняйте, только если прокси не подключается.
              </p>
            </template>
          </div>

          <div class="tgws-section">
            <div class="tgws-section-head">
              <span class="field-label">Производительность</span>
              <span class="muted">буфер и пул соединений</span>
            </div>
            <p v-if="!limitsKnown" class="tgws-hint">
              Не удалось прочитать настройки — обновите приложение.
            </p>
            <template v-else>
              <div class="tgws-fields">
                <label class="field">
                  <span class="field-label">Буфер, КБ</span>
                  <input
                    v-model="bufDraft"
                    class="tgws-input"
                    type="number"
                    :min="TGWS_BUF_MIN_KB"
                    :max="TGWS_BUF_MAX_KB"
                    @change="commitLimits"
                    @keydown.enter="commitLimits"
                  />
                </label>
                <label class="field">
                  <span class="field-label">Пул на DC</span>
                  <input
                    v-model="poolDraft"
                    class="tgws-input"
                    type="number"
                    :min="TGWS_POOL_MIN"
                    :max="TGWS_POOL_MAX"
                    @change="commitLimits"
                    @keydown.enter="commitLimits"
                  />
                </label>
              </div>
              <p v-if="limitsError" class="tgws-error">
                <AlertTriangle :size="14" />
                <span>{{ limitsError }}</span>
              </p>
              <p class="tgws-hint">
                Буфер — от {{ TGWS_BUF_MIN_KB }} до {{ TGWS_BUF_MAX_KB }} КБ, пул — от
                {{ TGWS_POOL_MIN }} до {{ TGWS_POOL_MAX }} готовых соединений на DC. Значения
                сохраняются при уходе из поля или по Enter; больше — не всегда быстрее.
              </p>
            </template>
          </div>

          <div class="tgws-section">
            <div class="tgws-section-head">
              <span class="field-label">Для специалистов</span>
              <span class="muted">{{
                engine === "rs" ? "часть флагов не работает на новом движке" : "редкие флаги"
              }}</span>
            </div>
            <p v-if="!flagsKnown" class="tgws-hint">
              Не удалось прочитать флаги — обновите приложение.
            </p>
            <template v-else>
              <div class="tgws-balance">
                <Switch
                  :model-value="verbose"
                  :disabled="tgwsBusy"
                  aria-label="Подробный лог"
                  @update:model-value="toggleExpertFlag({ verbose: $event })"
                />
                <span>
                  <strong>Подробный лог</strong>
                  <span class="muted">
                    Больше строк в логе прокси — помогает разобрать сбой, но файл растёт быстрее.
                  </span>
                </span>
              </div>

              <div class="tgws-balance">
                <Switch
                  :model-value="noSecure"
                  :disabled="tgwsBusy"
                  aria-label="Работать без TLS"
                  @update:model-value="toggleExpertFlag({ no_secure: $event })"
                />
                <span>
                  <strong>Без TLS</strong>
                  <span class="muted">
                    Соединение с Cloudflare без шифрования. У нового движка отключает TLS.
                    Включайте, только если обычное подключение не проходит.
                  </span>
                </span>
              </div>
              <p v-if="noSecure" class="tgws-warning">
                <AlertTriangle :size="14" />
                <span>Трафик до Cloudflare не шифруется — выключите, когда прокси заработает.</span>
              </p>

              <div class="tgws-balance">
                <Switch
                  :model-value="forceTestDc"
                  :disabled="tgwsBusy"
                  aria-label="Направить весь трафик на тестовые DC"
                  @update:model-value="toggleExpertFlag({ force_test_dc: $event })"
                />
                <span>
                  <strong>
                    Force test DC
                    <span class="tgws-tag">только старый движок</span>
                  </strong>
                  <span class="muted">
                    Весь трафик — на тестовые серверы Telegram. Обычные аккаунты через прокси
                    работать перестанут.
                  </span>
                </span>
              </div>

              <div class="tgws-balance">
                <Switch
                  :model-value="proxyProtocol"
                  :disabled="tgwsBusy"
                  aria-label="Принимать PROXY protocol"
                  @update:model-value="toggleExpertFlag({ proxy_protocol: $event })"
                />
                <span>
                  <strong>
                    PROXY protocol
                    <span class="tgws-tag">только старый движок</span>
                  </strong>
                  <span class="muted">
                    Прокси стоит за внешним сервером и получает адрес клиента из заголовка. Обычно
                    не нужно.
                  </span>
                </span>
              </div>

              <p v-if="expertFlagsError" class="tgws-error">
                <AlertTriangle :size="14" />
                <span>{{ expertFlagsError }}</span>
              </p>
            </template>
          </div>

          <div class="tgws-section">
            <div class="tgws-section-head">
              <span class="field-label">Адрес и ключ</span>
              <span class="muted">локальный вход в прокси</span>
            </div>
            <div class="tgws-fields">
              <label class="field">
                <span class="field-label">Хост</span>
                <input
                  v-model="hostDraft"
                  class="tgws-input"
                  spellcheck="false"
                  placeholder="127.0.0.1"
                  @change="commitHost"
                  @keydown.enter="commitHost"
                />
              </label>
              <label class="field">
                <span class="field-label">Порт</span>
                <input
                  v-model.number="portDraft"
                  class="tgws-input"
                  type="number"
                  min="1024"
                  max="65535"
                  @change="commitPort"
                  @keydown.enter="commitPort"
                />
              </label>
              <label class="field field-wide">
                <span class="field-label">Секретный ключ (32 hex)</span>
                <input
                  v-model="secretDraft"
                  class="tgws-input tgws-mono"
                  spellcheck="false"
                  maxlength="32"
                  @change="commitSecret"
                  @keydown.enter="commitSecret"
                />
              </label>

              <p class="tgws-hint field-wide">
                Изменения перезапускают прокси, если он включён. Ключ и ссылка не меняются, поэтому
                добавлять прокси в Telegram заново не нужно.
              </p>

              <p v-if="publicBinding" class="tgws-warning field-wide">
                <AlertTriangle :size="14" />
                <span>
                  Хост <strong>{{ displayHost(config?.tgws.host) }}</strong> открывает прокси другим
                  устройствам в сети. Оставьте 127.0.0.1, если доступ извне не нужен.
                </span>
              </p>
            </div>
          </div>

          <div class="tgws-log-head">
            <div>
              <span class="field-label">Логи</span>
              <span class="muted">Последние {{ tgwsLogs.length }} строк от процесса прокси</span>
            </div>
            <Button variant="ghost" size="sm" @click="logsOpen = !logsOpen">
              {{ logsOpen ? "Свернуть" : "Показать" }}
            </Button>
          </div>
          <pre v-if="logsOpen" ref="logBox" class="tgws-log">{{
            logText || "Пока пусто — включите прокси."
          }}</pre>
        </div>
      </details>
    </Card>
  </div>
</template>

<style scoped>
.tgws-page {
  padding-bottom: 8px;
}

/* Same glass treatment as the panels on the overview and settings pages. */
.tgws-card {
  border-color: var(--glass-border);
  background: linear-gradient(160deg, var(--glass-bg), var(--glass-bg-2));
  box-shadow:
    0 18px 50px var(--glass-shadow),
    inset 0 1px 0 var(--glass-highlight);
  backdrop-filter: blur(18px) saturate(135%);
}

/* The proxy is running, but no Telegram client has connected yet — and the
   reverse case, when Telegram sits on the VPN tunnel instead. */
.tgws-prompt {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 14px;
  border: 1px solid color-mix(in oklab, var(--warn) 45%, transparent);
  border-radius: 12px;
  background: var(--warn-soft);
}

.tgws-prompt-icon {
  display: grid;
  place-items: center;
  width: 34px;
  height: 34px;
  flex: 0 0 auto;
  border-radius: 10px;
  background: var(--accent-soft);
  color: var(--accent-text);
}

.tgws-prompt-text {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.tgws-prompt-text strong {
  color: var(--text);
  font-size: 13px;
}

.tgws-prompt-text span {
  color: var(--text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.tgws-hero-head {
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 12px;
}

.tgws-hero-icon {
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

.tgws-hero-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.tgws-hero-controls {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-left: auto;
}

.tgws-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-3);
}

.tgws-dot.is-ok {
  background: var(--ok);
  box-shadow: 0 0 0 3px var(--ok-soft);
}

.tgws-dot.is-warn {
  background: var(--warn);
  box-shadow: 0 0 0 3px var(--warn-soft);
}

.tgws-dot.is-danger {
  background: var(--danger);
  box-shadow: 0 0 0 3px var(--danger-soft);
}

.tgws-hero-content {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.tgws-meta {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
  gap: 8px;
}

.tgws-meta div {
  min-width: 0;
  padding: 9px 10px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--muted);
}

.tgws-meta span {
  display: block;
  color: var(--text-3);
  font-size: 11.5px;
  font-weight: 500;
  letter-spacing: 0;
  text-transform: none;
}

.tgws-meta strong {
  display: block;
  overflow: hidden;
  color: var(--text);
  font-family: var(--mono);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tgws-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.tgws-diag-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.diag-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.diag-row {
  display: flex;
  align-items: baseline;
  gap: 8px;
  color: var(--text-2);
  font-size: 12.5px;
}

.diag-row > span:first-child {
  width: 14px;
  color: var(--ok);
  font-weight: 700;
}

.diag-row.bad > span:first-child {
  color: var(--danger);
}

.diag-row.is-neutral > span:first-child {
  color: var(--text-3);
}

.diag-hint {
  margin: 0;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--surface-2);
  color: var(--text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.diag-hint.is-ok {
  border-color: color-mix(in oklab, var(--ok) 40%, transparent);
  background: var(--ok-soft);
  color: var(--ok);
}

.diag-hint.is-port,
.diag-hint.is-stopped,
.diag-hint.is-blocked {
  border-color: color-mix(in oklab, var(--warn) 40%, transparent);
  background: var(--warn-soft);
  color: var(--warn);
}

.tgws-diag-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

.tgws-progress {
  color: var(--text-2);
  font-size: 12px;
}

.tgws-error,
.tgws-warning {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 12px;
  border-radius: 9px;
  font-size: 12px;
  line-height: 1.45;
}

.tgws-error {
  border: 1px solid color-mix(in oklab, var(--danger) 40%, transparent);
  background: var(--danger-soft);
  color: var(--danger);
}

.tgws-error-body {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 8px;
  min-width: 0;
}

.tgws-warning {
  border: 1px solid color-mix(in oklab, var(--warn) 40%, transparent);
  background: var(--warn-soft);
  color: var(--warn);
}

.tgws-warning code {
  font-family: var(--mono);
  font-size: 11.5px;
}

/* Engine failure: louder than a warning, because the proxy does not start. */
.tgws-prompt.is-danger {
  border-color: color-mix(in oklab, var(--danger) 45%, transparent);
  background: var(--danger-soft);
}

.tgws-prompt.is-danger .tgws-prompt-icon {
  background: var(--danger-soft);
  color: var(--danger);
}

/* One settings group inside "Дополнительно": title, controls, hint. */
.tgws-section {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}

.tgws-section:first-child {
  padding-top: 0;
  border-top: 0;
}

.tgws-section-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
}

.tgws-section-head .muted {
  font-size: 11.5px;
}

/* A titled block inside a group: own domains, Worker list, FakeTLS. */
.tgws-sub {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

/* Two engines side by side: one tap switches, nothing is hidden in a menu. */
.tgws-choice {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
  gap: 8px;
}

.tgws-choice-option {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--secondary);
  color: var(--text-2);
  font: inherit;
  text-align: left;
  cursor: pointer;
}

.tgws-choice-option strong {
  color: var(--text);
  font-size: 12.5px;
}

.tgws-choice-option span {
  color: var(--text-3);
  font-size: 11.5px;
  line-height: 1.4;
}

.tgws-choice-option:hover:not(:disabled) {
  border-color: var(--primary);
}

.tgws-choice-option.is-active {
  border-color: var(--primary);
  background: var(--accent-soft);
}

.tgws-choice-option.is-active span {
  color: var(--text-2);
}

.tgws-choice-option:disabled {
  cursor: default;
  opacity: 0.55;
}

.tgws-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.tgws-chip {
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

.tgws-chip-remove {
  display: grid;
  place-items: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: 0;
  border-radius: 50%;
  background: transparent;
  color: var(--text-3);
  font: inherit;
  font-size: 14px;
  line-height: 1;
  cursor: pointer;
}

.tgws-chip-remove:hover:not(:disabled) {
  background: var(--muted);
  color: var(--text);
}

.tgws-chip-remove:disabled {
  cursor: default;
  opacity: 0.5;
}

.tgws-domain-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.tgws-domain-row .tgws-input {
  flex: 1 1 auto;
  min-width: 0;
}

.tgws-balance {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 9px 10px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--muted);
}

.tgws-balance > span {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.tgws-balance strong {
  color: var(--text);
  font-size: 12.5px;
}

.tgws-balance .muted {
  font-size: 11.5px;
  line-height: 1.4;
}

/* Marks a flag the lightweight engine ignores. */
.tgws-tag {
  display: inline-block;
  margin-left: 6px;
  padding: 1px 6px;
  border: 1px solid var(--border);
  border-radius: 999px;
  background: var(--muted);
  color: var(--text-3);
  font-size: 10.5px;
  font-weight: 500;
  vertical-align: 1px;
  white-space: nowrap;
}

.diag-note {
  margin: 0;
  color: var(--text-3);
  font-size: 12px;
  line-height: 1.45;
}

.diag-note strong {
  color: var(--text-2);
  font-weight: 600;
}

.tgws-update-content {
  display: flex;
  align-items: center;
  gap: 12px;
  /* CardContent defaults to pt-0 (it normally follows a header); this banner
     has no header, so it needs its own vertical rhythm. */
  padding: 14px 16px;
}

.tgws-update-icon {
  display: grid;
  place-items: center;
  width: 34px;
  height: 34px;
  flex: 0 0 auto;
  border-radius: 10px;
  background: var(--accent-soft);
  color: var(--accent-text);
}

.tgws-update-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.tgws-update-text strong {
  font-size: 12.5px;
}

.tgws-update-text span {
  color: var(--text-3);
  font-size: 12px;
}

.tgws-update-link,
.tgws-doc-link {
  align-self: flex-start;
  padding: 0;
  border: 0;
  background: none;
  color: var(--accent-text);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
}

.tgws-update-link:hover,
.tgws-doc-link:hover {
  text-decoration: underline;
}

.tgws-update-actions {
  display: flex;
  gap: 8px;
  margin-left: auto;
}

.tgws-connect {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.tgws-link-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.tgws-link {
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

.tgws-hint {
  color: var(--text-3);
  font-size: 12px;
  line-height: 1.5;
}

.tgws-hint code {
  color: var(--text-2);
  font-family: var(--mono);
  font-size: 11.5px;
}

.tgws-fields {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 140px);
  gap: 12px;
}

.tgws-fields .field-wide {
  grid-column: 1 / -1;
}

.tgws-input {
  width: 100%;
  height: 34px;
  padding: 0 10px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--secondary);
  color: var(--text);
  font: inherit;
  font-size: 12px;
}

.tgws-input::placeholder {
  color: var(--text-3);
}

.tgws-input:focus-visible {
  outline: none;
  border-color: var(--primary);
  box-shadow: var(--focus-ring);
}

.tgws-mono {
  font-family: var(--mono);
  font-size: 12px;
}

.tgws-log-head > div {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.tgws-log-head {
  display: flex;
  flex-direction: row;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

.tgws-advanced > summary {
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

.tgws-advanced > summary::-webkit-details-marker {
  display: none;
}

.tgws-advanced > summary::before {
  content: "▸";
  color: var(--text-3);
  font-weight: 400;
}

.tgws-advanced[open] > summary::before {
  content: "▾";
}

.tgws-advanced > summary .muted {
  font-size: var(--fs-caption);
  font-weight: 400;
}

.tgws-advanced-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 0 16px 16px;
}

.tgws-log {
  max-height: 240px;
  margin: 0;
  overflow: auto;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--secondary);
  color: var(--text-2);
  font-family: var(--mono);
  font-size: 12px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
}

@media (max-width: 860px) {
  .tgws-meta {
    grid-template-columns: minmax(0, 1fr);
  }

  .tgws-fields {
    grid-template-columns: minmax(0, 1fr);
  }

  .tgws-link-row {
    flex-wrap: wrap;
  }

  .tgws-domain-row {
    flex-wrap: wrap;
  }

  .tgws-prompt {
    flex-wrap: wrap;
  }
}
</style>
