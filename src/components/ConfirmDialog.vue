<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { DialogContent, DialogOverlay, DialogPortal, DialogRoot, DialogTitle } from "reka-ui";
import Button from "./ui/button/Button.vue";
import { confirmState, resolveConfirm } from "../lib/confirm";

const confirmButton = ref<{ focus: () => void } | null>(null);

function onOpenChange(open: boolean) {
  if (!open) resolveConfirm(false);
}

watch(
  () => confirmState.open,
  async (open) => {
    if (!open) return;
    await nextTick();
    confirmButton.value?.focus();
  },
);
</script>

<template>
  <DialogRoot :open="confirmState.open" @update:open="onOpenChange">
    <DialogPortal>
      <DialogOverlay class="modal-backdrop" />
      <DialogContent class="modal dialog-center" :aria-label="confirmState.title">
        <DialogTitle as-child>
          <h3>{{ confirmState.title }}</h3>
        </DialogTitle>
        <p v-if="confirmState.text" class="modal-text">{{ confirmState.text }}</p>

        <label v-if="confirmState.checkboxLabel" class="check-line modal-check">
          <input v-model="confirmState.checked" type="checkbox" />
          <span>{{ confirmState.checkboxLabel }}</span>
        </label>

        <div class="modal-actions">
          <Button variant="secondary" @click="resolveConfirm(false)">
            {{ confirmState.cancelLabel }}
          </Button>
          <Button
            ref="confirmButton"
            :variant="confirmState.danger ? 'destructive' : 'default'"
            @click="resolveConfirm(true)"
          >
            {{ confirmState.confirmLabel }}
          </Button>
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
