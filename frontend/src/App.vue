<script setup lang="ts">
import { onMounted, ref } from 'vue'

/** 描述 Web API 健康接口返回的数据。 */
interface HealthResponse {
  status: string
}

const apiState = ref<'checking' | 'online' | 'offline'>('checking')
const checkedAt = ref('')

/** 查询 Web API 健康状态并记录最近检查时间。 */
async function refreshStatus(): Promise<void> {
  apiState.value = 'checking'
  try {
    const response = await fetch('/api/health')
    if (!response.ok) throw new Error('health request failed')
    const result = (await response.json()) as HealthResponse
    apiState.value = result.status === 'ok' ? 'online' : 'offline'
  } catch {
    apiState.value = 'offline'
  }
  checkedAt.value = new Date().toLocaleTimeString()
}

onMounted(refreshStatus)
</script>

<template>
  <div class="app-shell">
    <header class="topbar">
      <a class="brand" href="/" aria-label="HXX 工作台首页">HXX</a>
      <span class="product-name">工作台</span>
    </header>

    <main class="content">
      <div class="page-heading">
        <div>
          <p class="eyebrow">系统</p>
          <h1>服务状态</h1>
        </div>
        <button class="refresh-button" type="button" @click="refreshStatus">
          刷新状态
        </button>
      </div>

      <section class="status-row" aria-live="polite">
        <div class="status-copy">
          <span class="status-label">Web API</span>
          <span class="status-value" :data-state="apiState">
            <span class="status-dot" aria-hidden="true"></span>
            {{ apiState === 'checking' ? '检查中' : apiState === 'online' ? '正常' : '不可用' }}
          </span>
        </div>
        <time v-if="checkedAt" class="checked-time">最近检查 {{ checkedAt }}</time>
      </section>
    </main>
  </div>
</template>
