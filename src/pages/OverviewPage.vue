<script setup lang="ts">
import { computed } from "vue";
import { useRouter } from "vue-router";
import { ArrowUpRight, Send, ShieldCheck, Zap } from "lucide-vue-next";
import Button from "../components/ui/button/Button.vue";
import Card from "../components/ui/card/Card.vue";
import CardContent from "../components/ui/card/CardContent.vue";
import CardDescription from "../components/ui/card/CardDescription.vue";
import CardHeader from "../components/ui/card/CardHeader.vue";
import CardTitle from "../components/ui/card/CardTitle.vue";
import Switch from "../components/ui/switch/Switch.vue";
import {
  appUpdate,
  benchDate,
  benchSummary,
  benchRunning,
  busy,
  checkAllUpdates,
  checkingAppUpdate,
  checkingUpdate,
  config,
  running,
  runtimeReady,
  status,
  statusText,
  statusTone,
  tgwsBusy,
  tgwsChecking,
  tgwsRunning,
  tgwsStateText,
  tgwsStatus,
  tgwsUpdate,
  toggleStart,
  toggleTgws,
  toggleXray,
  updateInfo,
  xrayBusy,
  xrayStatus,
} from "../store";
import { tgwsEndpoint } from "../lib/tgws";

const router = useRouter();

const zapretState = computed(() => {
  if (!runtimeReady.value) return "Не установлен";
  return running.value ? "Включён" : "Выключен";
});

const bestScore = computed(() => {
  const best = benchSummary.value?.best;
  if (!best) return null;
  return benchSummary.value?.results.find((result) => result.name === best)?.score ?? null;
});

const appHasUpdate = computed(() => !!appUpdate.value?.available);
const zapretHasUpdate = computed(() => !!updateInfo.value?.update_available);
const tgwsHasUpdate = computed(() => !!tgwsUpdate.value?.has_update);
const checkingAllUpdates = computed(
  () => checkingAppUpdate.value || checkingUpdate.value || tgwsChecking.value,
);

const updatesAvailable = computed(
  () => [appHasUpdate.value, zapretHasUpdate.value, tgwsHasUpdate.value].filter(Boolean).length,
);
const updatesLabel = computed(() => {
  const count = updatesAvailable.value;
  if (!count) return "Обновлений нет";
  return count === 1 ? "Доступно 1 обновление" : `Доступно обновлений: ${count}`;
});

const vpnRunning = computed(() => xrayStatus.value?.state === "running");
const activeServices = computed(
  () =>
    [runtimeReady.value && running.value, tgwsRunning.value, vpnRunning.value].filter(Boolean)
      .length,
);
const heroHint = computed(() =>
  activeServices.value ? `Активно сервисов: ${activeServices.value} из 3` : "Все сервисы выключены",
);

const telegramEndpoint = computed(() =>
  tgwsEndpoint(config.value?.tgws.host, config.value?.tgws.port),
);
const vpnHint = computed(() => {
  if (!xrayStatus.value) return "Загрузка…";
  if (!vpnRunning.value) return "VPN выключен";
  const connections = xrayStatus.value.client_connections ?? 0;
  return connections > 0 ? "Telegram через VPN" : "VPN подключён";
});
const vpnProfile = computed(() => xrayStatus.value?.profile_name || "не выбрана");
const vpnEndpoint = computed(() => `127.0.0.1:${xrayStatus.value?.socks_port ?? 17808}`);

const telegramHint = computed(() => {
  if (!tgwsRunning.value) return tgwsStateText.value;
  return (tgwsStatus.value?.client_connections_total ?? 0) > 0
    ? "Прокси включён"
    : "Прокси включён · Telegram не подключён";
});
</script>

<template>
  <div class="main-page">
    <Card class="home-hero">
      <div class="home-hero-content">
        <span class="status-dot" :class="statusTone" aria-hidden="true"></span>
        <div class="home-hero-text">
          <strong>{{ statusText }}</strong>
          <span class="muted">{{ heroHint }}</span>
        </div>
        <Button
          v-if="updatesAvailable"
          variant="secondary"
          size="sm"
          @click="router.push('/settings')"
        >
          {{ updatesLabel }}
        </Button>
      </div>
    </Card>

    <div class="power-grid">
      <Card class="power-card">
        <CardHeader class="power-head">
          <div class="power-icon zapret"><ShieldCheck :size="20" /></div>
          <div class="power-heading">
            <CardTitle>Zapret</CardTitle>
            <CardDescription>{{ zapretState }}</CardDescription>
          </div>
          <Switch
            :model-value="running"
            :disabled="busy || benchRunning || !runtimeReady"
            aria-label="Включить или выключить обход Zapret"
            @update:model-value="toggleStart"
          />
        </CardHeader>
        <CardContent class="power-content">
          <div class="power-meta">
            <span>Стратегия</span>
            <strong>{{ status?.current_strategy || "не выбрана" }}</strong>
          </div>
          <Button
            v-if="!runtimeReady"
            class="power-action"
            variant="secondary"
            size="sm"
            @click="router.push('/zapret')"
          >
            Установить Zapret
          </Button>
          <Button
            v-else
            class="power-action"
            variant="outline"
            size="sm"
            @click="router.push('/zapret')"
          >
            Стратегии и проверка
            <ArrowUpRight :size="13" />
          </Button>
        </CardContent>
      </Card>

      <Card class="power-card">
        <CardHeader class="power-head">
          <div class="power-icon telegram"><Send :size="19" /></div>
          <div class="power-heading">
            <CardTitle>Telegram</CardTitle>
            <CardDescription>{{ telegramHint }}</CardDescription>
          </div>
          <Switch
            :model-value="tgwsRunning"
            :disabled="tgwsBusy"
            aria-label="Включить или выключить Telegram WS-прокси"
            @update:model-value="toggleTgws"
          />
        </CardHeader>
        <CardContent class="power-content">
          <div class="power-meta">
            <span>Адрес</span>
            <strong>{{ telegramEndpoint }}</strong>
          </div>
          <Button
            class="power-action"
            variant="outline"
            size="sm"
            @click="router.push('/telegram')"
          >
            Подключение и ключ
            <ArrowUpRight :size="13" />
          </Button>
        </CardContent>
      </Card>

      <Card class="power-card">
        <CardHeader class="power-head">
          <div class="power-icon telegram"><Zap :size="19" /></div>
          <div class="power-heading">
            <CardTitle>VPN</CardTitle>
            <CardDescription>{{ vpnHint }}</CardDescription>
          </div>
          <Switch
            :model-value="vpnRunning"
            :disabled="xrayBusy"
            aria-label="Включить или выключить VPN"
            @update:model-value="toggleXray"
          />
        </CardHeader>
        <CardContent class="power-content">
          <div class="power-meta">
            <span>Локация</span>
            <strong>{{ vpnProfile }}</strong>
          </div>
          <div class="power-meta">
            <span>SOCKS5</span>
            <strong>{{ vpnEndpoint }}</strong>
          </div>
          <Button class="power-action" variant="outline" size="sm" @click="router.push('/vpn')">
            Подписка и локации
            <ArrowUpRight :size="13" />
          </Button>
        </CardContent>
      </Card>
    </div>

    <div class="home-rows">
      <Card class="home-row">
        <div class="home-row-main">
          <span class="eyebrow">Обновления</span>
          <strong>{{ updatesLabel }}</strong>
        </div>
        <div class="home-row-actions">
          <Button
            variant="outline"
            size="sm"
            :disabled="checkingAllUpdates || benchRunning"
            @click="checkAllUpdates"
          >
            {{ checkingAllUpdates ? "Проверка…" : "Проверить" }}
          </Button>
          <Button variant="ghost" size="sm" @click="router.push('/settings')">Настройки</Button>
        </div>
      </Card>

      <Card v-if="benchSummary" class="home-row">
        <div class="home-row-main">
          <span class="eyebrow">Последняя проверка</span>
          <strong>{{ benchSummary.best || "Без победителя" }}</strong>
        </div>
        <div class="home-row-actions">
          <span class="muted">Балл {{ bestScore ?? "—" }} · {{ benchDate }}</span>
          <Button variant="ghost" size="sm" @click="router.push('/zapret')">
            К стратегиям
            <ArrowUpRight :size="13" />
          </Button>
        </div>
      </Card>
    </div>
  </div>
</template>
