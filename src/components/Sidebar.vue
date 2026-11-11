<script setup lang="ts">
import { ref } from "vue";
import { RouterLink } from "vue-router";
import {
  LayoutDashboard,
  PanelLeftClose,
  PanelLeftOpen,
  Send,
  Settings2,
  ShieldCheck,
} from "lucide-vue-next";
import logo from "../assets/logo.png";
import { appVersion } from "../store";

const collapsed = ref(false);

const items = [
  { to: "/", id: "dashboard", label: "Главная", icon: LayoutDashboard },
  { to: "/zapret", id: "zapret", label: "Zapret", icon: ShieldCheck },
  { to: "/telegram", id: "telegram", label: "Telegram", icon: Send },
  { to: "/settings", id: "settings", label: "Настройки", icon: Settings2 },
];
</script>

<template>
  <aside class="sidebar" :class="{ collapsed }" aria-label="Основная навигация">
    <div class="sidebar-brand">
      <img class="brand-icon" :src="logo" alt="" aria-hidden="true" />
      <div v-if="!collapsed" class="brand-text">
        <strong>Splitline</strong>
      </div>
    </div>

    <nav class="sidebar-nav">
      <RouterLink
        v-for="item in items"
        :key="item.to"
        :to="item.to"
        class="nav-item"
        :data-icon="item.id"
        active-class=""
        exact-active-class="active"
        :title="collapsed ? item.label : undefined"
      >
        <component :is="item.icon" :size="18" :stroke-width="1.9" />
        <span v-if="!collapsed" class="nav-label">{{ item.label }}</span>
      </RouterLink>
    </nav>

    <div class="sidebar-bottom">
      <div v-if="!collapsed" class="sidebar-version">версия {{ appVersion || "1.0.0" }}</div>
      <button
        class="collapse-btn"
        type="button"
        :aria-label="collapsed ? 'Развернуть меню' : 'Свернуть меню'"
        @click="collapsed = !collapsed"
      >
        <PanelLeftOpen v-if="collapsed" :size="17" />
        <PanelLeftClose v-else :size="17" />
        <span v-if="!collapsed">Свернуть</span>
      </button>
    </div>
  </aside>
</template>
