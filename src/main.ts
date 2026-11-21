import { createApp } from "vue";
import "@fontsource-variable/inter";
import App from "./App.vue";
import router from "./router";
import { applyTheme, readCachedTheme } from "./store/actions";
import "./style.css";

// The theme is applied before the first render: the backend call in `initStore`
// only confirms it, so a "Кармин" user never sees a frame of the default theme.
// The cache is written by `applyTheme`, so a cold start paints the same colours
// the previous session ended with.
applyTheme(readCachedTheme());

createApp(App).use(router).mount("#app");
