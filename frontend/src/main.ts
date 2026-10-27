import { createApp } from 'vue'
import {
  ElButton,
  ElDialog,
  ElEmpty,
  ElInput,
  ElTable,
  ElTableColumn,
  ElTag,
} from 'element-plus'
import { DataAnalysis, Loading, Plus, Promotion, Refresh, User } from '@element-plus/icons-vue'
import App from './App.vue'
import 'element-plus/dist/index.css'
import './style.css'

const app = createApp(App)
const components = { ElButton, ElDialog, ElEmpty, ElInput, ElTable, ElTableColumn, ElTag }
const icons = { DataAnalysis, Loading, Plus, Promotion, Refresh, User }
for (const [name, component] of Object.entries({ ...components, ...icons })) {
  app.component(name, component)
}
app.mount('#app')
