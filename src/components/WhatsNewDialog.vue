<script setup lang="ts">
import { computed } from "vue";
import { DialogContent, DialogOverlay, DialogPortal, DialogRoot, DialogTitle } from "reka-ui";
import MarkdownBody from "./MarkdownBody.vue";
import Button from "./ui/button/Button.vue";
import { dismissWhatsNew, openReleasePage, whatsNew } from "../store";

const open = computed(() => !!whatsNew.value);

function onOpenChange(value: boolean) {
  if (!value) dismissWhatsNew();
}
</script>

<template>
  <DialogRoot :open="open" @update:open="onOpenChange">
    <DialogPortal>
      <DialogOverlay class="modal-backdrop" />
      <DialogContent
        class="modal whats-new-modal dialog-center"
        :aria-label="`Что нового в Splitline ${whatsNew?.version ?? ''}`"
      >
        <DialogTitle as-child>
          <h3>Что нового в Splitline {{ whatsNew?.version }}</h3>
        </DialogTitle>
        <MarkdownBody v-if="whatsNew" class="whats-new-notes" :source="whatsNew.notes" />

        <div class="modal-actions">
          <Button v-if="whatsNew" variant="secondary" @click="openReleasePage(whatsNew.url)">
            Открыть релиз
          </Button>
          <Button variant="secondary" @click="dismissWhatsNew">Закрыть</Button>
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
