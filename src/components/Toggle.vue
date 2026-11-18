<script setup lang="ts">
import Switch from "./ui/switch/Switch.vue";

defineProps<{
  label: string;
  hint?: string;
  checked?: boolean;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  change: [value: boolean];
}>();
</script>

<template>
  <div class="toggle-row" :class="{ disabled }">
    <span class="toggle-text">
      <span class="toggle-label">{{ label }}</span>
      <span v-if="hint" class="toggle-hint">{{ hint }}</span>
    </span>
    <Switch
      :model-value="checked"
      :disabled="disabled"
      :aria-label="label"
      @update:model-value="emit('change', $event)"
    />
  </div>
</template>
