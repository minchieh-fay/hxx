<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { ElMessage } from 'element-plus'
import MarkdownIt from 'markdown-it'

/** 描述当前浏览器对应的用户身份和最近访问 IP。 */
interface Profile {
  client_id: string
  name: string
  client_ip: string
}

/** 描述侧栏和会话页使用的会话元数据。 */
interface Session {
  id: string
  title: string
  created_at: string
  updated_at: string
  working: boolean
  last_error: string | null
}

/** 描述会话历史中的一条消息。 */
interface Message {
  id: number
  role: 'user' | 'assistant'
  content: string
  created_at: string
}

/** 描述待上传文件及其可选的本地图片预览地址。 */
interface PendingAttachment {
  file: File
  previewUrl: string | null
}

/** 描述会话详情 API 返回的数据。 */
interface SessionResponse {
  session: Session
  messages: Message[]
  activities: Activity[]
}

/** 描述 Codex 当前轮次中正在运行或已经完成的工具活动。 */
interface Activity {
  id: number
  source_id: string
  kind: string
  title: string
  detail: string
  status: string
  created_at: string
  updated_at: string
}

/** 描述一个统计时间范围的请求和 token 总量。 */
interface UsageTotals {
  requests: number
  input_tokens: number
  output_tokens: number
}

/** 描述仪表盘中的单个浏览器用户排名。 */
interface UsageRank extends UsageTotals {
  client_id: string
  name: string
}

/** 描述仪表盘日、周、月汇总和用户排名。 */
interface Dashboard {
  today: UsageTotals
  week: UsageTotals
  month: UsageTotals
  rankings: UsageRank[]
}

const identityKey = 'hxx-client-id'
const nameKey = 'hxx-client-name'
let clientId = localStorage.getItem(identityKey)
if (!clientId) {
  clientId = createClientId()
  localStorage.setItem(identityKey, clientId)
}

const profile = ref<Profile | null>(null)
const sessions = ref<Session[]>([])
const activeSession = ref<SessionResponse | null>(null)
const dashboard = ref<Dashboard | null>(null)
const currentPath = ref(window.location.pathname)
const mode = ref<'chat' | 'editing' | 'preview'>('chat')
const isSending = ref(false)
const isInterrupting = ref(false)
const respondingApproval = ref<string | null>(null)
const loadingSession = ref(false)
const composerText = ref('')
const sessionAttachments = ref<PendingAttachment[]>([])
const composerAttachments = ref<PendingAttachment[]>([])
const sessionFileInput = ref<HTMLInputElement | null>(null)
const composerFileInput = ref<HTMLInputElement | null>(null)
const draggingFiles = ref(false)
const renameOpen = ref(false)
const renameValue = ref('')
const clockNow = ref(Date.now())
let refreshTimer: ReturnType<typeof setTimeout> | undefined
let clockTimer: ReturnType<typeof setInterval> | undefined
const form = reactive({ topic: '', environment: '', issue: '', work: '', notes: '' })
const markdown = new MarkdownIt({
  html: false,
  linkify: false,
  breaks: false,
})

/** 在 HTTP 内网环境也生成浏览器会话使用的 UUID。 */
function createClientId(): string {
  const bytes = new Uint8Array(16)
  if (typeof window.crypto?.getRandomValues === 'function') {
    window.crypto.getRandomValues(bytes)
  } else {
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Math.floor(Math.random() * 256)
    }
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40
  bytes[8] = (bytes[8] & 0x3f) | 0x80
  const hex = Array.from(bytes, (value) => value.toString(16).padStart(2, '0'))
  return [
    hex.slice(0, 4).join(''),
    hex.slice(4, 6).join(''),
    hex.slice(6, 8).join(''),
    hex.slice(8, 10).join(''),
    hex.slice(10, 16).join(''),
  ].join('-')
}

const previewMarkdown = computed(() => [
  `# 主题\n${form.topic}`,
  `# 环境描述\n${form.environment}`,
  `# 问题/需求描述\n${form.issue}`,
  `# 希望 Codex 完成的工作\n${form.work}`,
  `# 备注\n${form.notes}`,
].join('\n\n'))

/** 将会话正文转换为禁用原始 HTML 的安全 Markdown。 */
function renderMarkdown(content: string): string {
  return markdown.render(content)
}

/** 为当前浏览器身份添加请求头并解码 HXX API 响应。 */
async function api<T>(path: string, options: RequestInit = {}): Promise<T> {
  const headers = new Headers(options.headers)
  headers.set('X-Client-Id', clientId ?? '')
  headers.set('X-Client-Name', profile.value?.name ?? localStorage.getItem(nameKey) ?? clientId ?? '')
  if (options.body && !(options.body instanceof FormData)) {
    headers.set('Content-Type', 'application/json')
  }
  const response = await fetch(path, { ...options, headers })
  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: '请求失败' }))
    throw new Error(body.error ?? `HTTP ${response.status}`)
  }
  return response.json() as Promise<T>
}

/** 载入浏览器身份、会话列表及当前页面所需数据。 */
async function initialize(): Promise<void> {
  // step.1 恢复本地显示名称并向后端登记当前浏览器身份。
  try {
    profile.value = await api<Profile>('/api/profile')
    localStorage.setItem(nameKey, profile.value.name)
    sessions.value = await api<Session[]>('/api/sessions')

    // step.2 根据当前路径加载仪表盘或恢复最近会话。
    if (currentPath.value === '/dashboard') {
      await loadDashboard()
    } else if (sessions.value.length > 0) {
      await selectSession(sessions.value[0].id)
    }
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '连接 HXX 服务失败')
  }
}

/** 读取会话详情并切换到选中的对话。 */
async function selectSession(sessionId: string): Promise<void> {
  clearSessionRefresh()
  loadingSession.value = true
  try {
    activeSession.value = await api<SessionResponse>(`/api/sessions/${sessionId}`)
    mode.value = 'chat'
    isSending.value = activeSession.value.session.working
    if (isSending.value) scheduleSessionRefresh(sessionId)
    if (currentPath.value === '/dashboard') navigate('/')
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '无法载入会话')
  } finally {
    loadingSession.value = false
  }
}

/** 在首页进入新会话表单并清空上次填写内容。 */
function startNewSession(): void {
  clearSessionRefresh()
  navigate('/')
  activeSession.value = null
  isSending.value = false
  form.topic = ''
  form.environment = ''
  form.issue = ''
  form.work = ''
  form.notes = ''
  clearAttachments(sessionAttachments.value)
  sessionAttachments.value = []
  mode.value = 'editing'
}

/** 从确认预览创建远程线程并发送结构化 Markdown 首条消息。 */
async function confirmNewSession(): Promise<void> {
  // step.1 校验必填说明并向 HXX 提交新会话表单。
  if (!form.issue.trim() || !form.work.trim()) {
    ElMessage.warning('请填写问题/需求描述和希望 Codex 完成的工作')
    return
  }
  isSending.value = true
  try {
    const body = new FormData()
    Object.entries(form).forEach(([key, value]) => body.append(key, value))
    sessionAttachments.value.forEach(({ file }) => body.append('files', file))
    const result = await api<SessionResponse>('/api/sessions', {
      method: 'POST',
      body,
    })

    // step.2 更新会话列表并进入新会话对话页面。
    activeSession.value = result
    isSending.value = result.session.working
    sessions.value = await api<Session[]>('/api/sessions')
    mode.value = 'chat'
    clearAttachments(sessionAttachments.value)
    sessionAttachments.value = []
    if (isSending.value) scheduleSessionRefresh(result.session.id)
    ElMessage.success('会话已创建')
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '创建会话失败')
  } finally {
    isSending.value = activeSession.value?.session.working ?? false
  }
}

/** 发送普通消息并用后端持久化后的完整对话刷新界面。 */
async function sendMessage(): Promise<void> {
  // step.1 检查会话和消息输入，锁定当前提交内容。
  const text = composerText.value.trim()
  if (!activeSession.value || (!text && composerAttachments.value.length === 0) || isSending.value) return
  const sentAttachments = composerAttachments.value
  const previousSession = activeSession.value
  composerText.value = ''
  composerAttachments.value = []
  isSending.value = true
  activeSession.value = {
    ...previousSession,
    messages: [
      ...previousSession.messages,
      {
        id: Date.now(),
        role: 'user',
        content: `${text}${attachmentLabel(sentAttachments)}`,
        created_at: new Date().toISOString(),
      },
    ],
  }
  try {
    // step.2 调用会话线程并刷新最新回复与列表活动时间。
    const body = new FormData()
    body.append('text', text)
    sentAttachments.forEach(({ file }) => body.append('files', file))
    activeSession.value = await api<SessionResponse>(
      `/api/sessions/${previousSession.session.id}/turns`,
      { method: 'POST', body },
    )
    clearAttachments(sentAttachments)
    isSending.value = activeSession.value.session.working
    sessions.value = await api<Session[]>('/api/sessions')
    if (isSending.value) scheduleSessionRefresh(activeSession.value.session.id)
  } catch (error) {
    composerText.value = text
    composerAttachments.value = sentAttachments
    activeSession.value = previousSession
    ElMessage.error(error instanceof Error ? error.message : '发送消息失败')
  } finally {
    isSending.value = activeSession.value?.session.working ?? false
  }
}

/** 把选择、拖放或粘贴的文件加入指定附件队列。 */
function addAttachments(files: FileList | File[], target: 'session' | 'composer'): void {
  // step.1 合并用户提供的文件并跳过已经加入的同名同尺寸文件。
  const queue = target === 'session' ? sessionAttachments : composerAttachments
  const additions = Array.from(files)
    .filter((file) => !queue.value.some((item) => (
      item.file.name === file.name && item.file.size === file.size
    )))
    .map((file) => ({
      file,
      previewUrl: file.type.startsWith('image/') ? URL.createObjectURL(file) : null,
    }))
  // step.2 统一限制总上传体积并更新预览队列。
  const total = [...queue.value, ...additions].reduce((sum, item) => sum + item.file.size, 0)
  if (total > 80 * 1024 * 1024) {
    additions.forEach((item) => item.previewUrl && URL.revokeObjectURL(item.previewUrl))
    ElMessage.warning('附件总大小不能超过 80 MB')
    return
  }
  queue.value.push(...additions)
}

/** 将文件选择器选中的文件加入队列并清空控件值以支持重复选择。 */
function selectAttachments(event: Event, target: 'session' | 'composer'): void {
  const input = event.target as HTMLInputElement
  addAttachments(input.files ?? [], target)
  input.value = ''
}

/** 释放图片预览地址并从待发送队列移除指定文件。 */
function removeAttachment(target: 'session' | 'composer', index: number): void {
  const queue = target === 'session' ? sessionAttachments : composerAttachments
  const [removed] = queue.value.splice(index, 1)
  if (removed?.previewUrl) URL.revokeObjectURL(removed.previewUrl)
}

/** 释放附件队列中的临时图片预览地址。 */
function clearAttachments(items: PendingAttachment[]): void {
  items.forEach((item) => item.previewUrl && URL.revokeObjectURL(item.previewUrl))
}

/** 为本地乐观消息生成可识别的附件清单。 */
function attachmentLabel(items: PendingAttachment[]): string {
  if (!items.length) return ''
  return `\n\n附件：${items.map(({ file }) => file.name).join('、')}`
}

/** 接收系统剪贴板中的图片并放入当前附件队列。 */
function pasteAttachments(event: ClipboardEvent, target: 'session' | 'composer'): void {
  const files = Array.from(event.clipboardData?.items ?? [])
    .filter((item) => item.kind === 'file')
    .map((item) => item.getAsFile())
    .filter((file): file is File => file !== null)
  if (files.length) addAttachments(files, target)
}

/** 处理拖放文件并在拖拽期间保持附件区域提示状态。 */
function dropAttachments(event: DragEvent, target: 'session' | 'composer'): void {
  draggingFiles.value = false
  const files = event.dataTransfer?.files
  if (files?.length) addAttachments(files, target)
}

/** 请求 App Server 中断当前会话正在运行的轮次。 */
async function interruptTurn(): Promise<void> {
  if (!activeSession.value || isInterrupting.value) return
  isInterrupting.value = true
  try {
    await api(`/api/sessions/${activeSession.value.session.id}/interrupt`, {
      method: 'POST',
    })
    ElMessage.info('已发送中断请求')
    scheduleSessionRefresh(activeSession.value.session.id)
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '中断请求失败')
  } finally {
    isInterrupting.value = false
  }
}

/** 将用户对当前工具审批的决定提交给 HXX 后端。 */
async function decideApproval(activity: Activity, decision: 'accept' | 'decline') {
  // step.1 锁定当前审批并提交决定，避免重复点击。
  if (!activeSession.value || respondingApproval.value) return
  const sessionId = activeSession.value.session.id
  respondingApproval.value = activity.source_id
  try {
    await api(`/api/sessions/${sessionId}/approvals`, {
      method: 'POST',
      body: JSON.stringify({ source_id: activity.source_id, decision }),
    })
    // step.2 继续刷新会话，直到 App Server 确认审批结果。
    scheduleSessionRefresh(sessionId)
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '提交审批决定失败')
  } finally {
    respondingApproval.value = null
  }
}

/** 载入统计摘要及本月用户用量排名。 */
async function loadDashboard(): Promise<void> {
  try {
    dashboard.value = await api<Dashboard>('/api/dashboard')
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '无法读取用量统计')
  }
}

/** 持续刷新当前会话，使文本、工具活动和运行状态秒级出现在对话框。 */
function scheduleSessionRefresh(sessionId: string): void {
  // step.1 保证同一会话只有一个轮询请求等待执行。
  if (refreshTimer) clearTimeout(refreshTimer)
  refreshTimer = setTimeout(async () => {
    if (activeSession.value?.session.id !== sessionId) return
    try {
      // step.2 合并最新消息和工具活动，再按服务端运行状态续订轮询。
      activeSession.value = await api<SessionResponse>(`/api/sessions/${sessionId}`)
      isSending.value = activeSession.value.session.working
      if (isSending.value) {
        scheduleSessionRefresh(sessionId)
      } else {
        sessions.value = await api<Session[]>('/api/sessions')
        if (activeSession.value.session.last_error) {
          ElMessage.error(activeSession.value.session.last_error)
        }
      }
    } catch (error) {
      isSending.value = false
      ElMessage.error(error instanceof Error ? error.message : '刷新会话失败')
    }
  }, 700)
}

/** 清理当前页面不再需要的会话轮询计时器。 */
function clearSessionRefresh(): void {
  if (refreshTimer) clearTimeout(refreshTimer)
  refreshTimer = undefined
}

/** 在工作台和统计页面之间切换浏览器路径。 */
function navigate(path: string): void {
  if (window.location.pathname !== path) history.pushState({}, '', path)
  currentPath.value = path
  if (path === '/dashboard') void loadDashboard()
}

/** 打开个人名称编辑窗口并预填当前显示名称。 */
function editName(): void {
  renameValue.value = profile.value?.name ?? clientId ?? ''
  renameOpen.value = true
}

/** 保存个人名称并更新浏览器本地缓存。 */
async function saveName(): Promise<void> {
  try {
    profile.value = await api<Profile>('/api/profile', {
      method: 'PATCH',
      body: JSON.stringify({ name: renameValue.value }),
    })
    localStorage.setItem(nameKey, profile.value.name)
    renameOpen.value = false
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '保存名称失败')
  }
}

/** 将接口时间转换成侧栏中紧凑的本地时间标签。 */
function formatTime(value: string): string {
  return new Date(value).toLocaleString(undefined, {
    month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit',
  })
}

/** 将 token 和调用次数格式化为适合表格阅读的数字。 */
function formatNumber(value: number): string {
  return new Intl.NumberFormat().format(value)
}

/** 显示会话或工具活动从开始到当前的秒数。 */
function elapsedSeconds(start: string, end?: string): string {
  const stop = end ? Date.parse(end) : clockNow.value
  const seconds = Math.max(0, Math.floor((stop - Date.parse(start)) / 1000))
  return `${seconds} 秒`
}

/** 根据活动状态计算运行时长或完成耗时。 */
function activityDuration(activity: Activity): string {
  const isActiveTurn = activeSession.value?.session.working ?? false
  return elapsedSeconds(
    activity.created_at,
    activity.status === 'running' && isActiveTurn ? undefined : activity.updated_at,
  )
}

/** 返回当前 Codex 轮次的持续运行时间。 */
function turnDuration(): string {
  const startedAt = activeSession.value?.session.updated_at
  return startedAt ? elapsedSeconds(startedAt) : '0 秒'
}

/** 处理浏览器前进和后退并加载当前页面数据。 */
function onPopState(): void {
  currentPath.value = window.location.pathname
  if (currentPath.value === '/dashboard') void loadDashboard()
}

onMounted(() => {
  window.addEventListener('popstate', onPopState)
  clockTimer = setInterval(() => { clockNow.value = Date.now() }, 500)
  void initialize()
})
onUnmounted(() => {
  window.removeEventListener('popstate', onPopState)
  clearSessionRefresh()
  if (clockTimer) clearInterval(clockTimer)
})
</script>

<template>
  <div class="app-shell">
    <header class="topbar">
      <a class="brand" href="/" @click.prevent="navigate('/')">HXX</a>
      <span class="product-name">工作台</span>
      <div class="topbar-actions">
        <el-button text @click="navigate('/dashboard')">
          <el-icon><DataAnalysis /></el-icon>
          用量统计
        </el-button>
        <el-button class="profile-button" text @click="editName">
          <el-icon><User /></el-icon>
          {{ profile?.name ?? '载入中' }}
        </el-button>
      </div>
    </header>

    <div class="workspace-layout">
      <aside class="sidebar" aria-label="会话列表">
        <div class="sidebar-heading">
          <span>会话</span>
          <el-button circle size="small" aria-label="新建会话" title="新建会话" @click="startNewSession">
            <el-icon><Plus /></el-icon>
          </el-button>
        </div>
        <el-button class="new-session-button" plain @click="startNewSession">
          <el-icon><Plus /></el-icon>
          新建会话
        </el-button>
        <nav class="session-list">
          <button
            v-for="session in sessions"
            :key="session.id"
            class="session-item"
            :class="{ active: activeSession?.session.id === session.id }"
            type="button"
            @click="selectSession(session.id)"
          >
            <span class="session-title">{{ session.title || '新会话' }}</span>
            <time>{{ formatTime(session.updated_at) }}</time>
          </button>
        </nav>
        <div class="sidebar-footer" v-if="profile">
          <span class="identity-dot"></span>
          <span class="identity-name">{{ profile.name }}</span>
          <span class="identity-ip">{{ profile.client_ip }}</span>
        </div>
      </aside>

      <main v-if="currentPath === '/dashboard'" class="main-panel dashboard-page">
        <div class="page-heading">
          <div>
            <p class="eyebrow">统计</p>
            <h1>用量统计</h1>
          </div>
          <el-button aria-label="刷新统计" title="刷新统计" @click="loadDashboard">
            <el-icon><Refresh /></el-icon>
          </el-button>
        </div>
        <template v-if="dashboard">
          <section class="stats-grid" aria-label="时间范围用量">
            <article class="stat-block">
              <span>今日</span>
              <strong>{{ formatNumber(dashboard.today.requests) }}</strong>
              <small>次请求</small>
              <div>
                {{ formatNumber(dashboard.today.input_tokens) }} 输入 /
                {{ formatNumber(dashboard.today.output_tokens) }} 输出
              </div>
            </article>
            <article class="stat-block">
              <span>本周</span>
              <strong>{{ formatNumber(dashboard.week.requests) }}</strong>
              <small>次请求</small>
              <div>
                {{ formatNumber(dashboard.week.input_tokens) }} 输入 /
                {{ formatNumber(dashboard.week.output_tokens) }} 输出
              </div>
            </article>
            <article class="stat-block">
              <span>本月</span>
              <strong>{{ formatNumber(dashboard.month.requests) }}</strong>
              <small>次请求</small>
              <div>
                {{ formatNumber(dashboard.month.input_tokens) }} 输入 /
                {{ formatNumber(dashboard.month.output_tokens) }} 输出
              </div>
            </article>
          </section>
          <section class="ranking-section">
            <div class="section-heading">
              <h2>用户排名</h2>
              <span>本月</span>
            </div>
            <el-table :data="dashboard.rankings" size="small" empty-text="暂无用量记录">
              <el-table-column type="index" label="#" width="56" />
              <el-table-column prop="name" label="用户" min-width="140" />
              <el-table-column prop="client_id" label="浏览器 ID" min-width="220" show-overflow-tooltip />
              <el-table-column label="请求" width="100" align="right">
                <template #default="scope">{{ formatNumber(scope.row.requests) }}</template>
              </el-table-column>
              <el-table-column label="输入 Token" width="130" align="right">
                <template #default="scope">{{ formatNumber(scope.row.input_tokens) }}</template>
              </el-table-column>
              <el-table-column label="输出 Token" width="130" align="right">
                <template #default="scope">{{ formatNumber(scope.row.output_tokens) }}</template>
              </el-table-column>
            </el-table>
          </section>
        </template>
        <el-empty v-else description="正在读取统计" />
      </main>

      <main v-else class="main-panel conversation-page">
        <section
          v-if="mode === 'editing'"
          class="session-form-page"
          :class="{ 'file-drop-active': draggingFiles }"
          @dragover.prevent="draggingFiles = true"
          @dragleave="draggingFiles = false"
          @drop.prevent="dropAttachments($event, 'session')"
          @paste="pasteAttachments($event, 'session')"
        >
          <div class="page-heading">
            <div>
              <p class="eyebrow">新会话</p>
              <h1>整理任务说明</h1>
            </div>
          </div>
          <div class="form-fields">
            <label class="form-field">
              <span>主题</span>
              <el-input v-model="form.topic" placeholder="例如：修复服务器漏洞" maxlength="120" />
            </label>
            <label class="form-field">
              <span>环境描述</span>
              <el-input
                v-model="form.environment"
                type="textarea"
                :autosize="{ minRows: 2, maxRows: 8 }"
                placeholder="系统、服务、约束和相关环境信息"
              />
            </label>
            <label class="form-field">
              <span>问题 / 需求描述 <b>*</b></span>
              <el-input
                v-model="form.issue"
                type="textarea"
                :autosize="{ minRows: 3, maxRows: 10 }"
                placeholder="描述当前问题、目标和已知线索"
              />
            </label>
            <label class="form-field">
              <span>希望 Codex 完成的工作 <b>*</b></span>
              <el-input
                v-model="form.work"
                type="textarea"
                :autosize="{ minRows: 3, maxRows: 10 }"
                placeholder="说明希望 Codex 执行或交付的内容"
              />
            </label>
            <label class="form-field">
              <span>备注</span>
              <el-input
                v-model="form.notes"
                type="textarea"
                :autosize="{ minRows: 2, maxRows: 8 }"
                placeholder="风险、限制或其他补充信息"
              />
            </label>
          </div>
          <input
            ref="sessionFileInput"
            class="file-input"
            type="file"
            multiple
            @change="selectAttachments($event, 'session')"
          />
          <div v-if="sessionAttachments.length" class="attachment-list">
            <div
              v-for="(item, index) in sessionAttachments"
              :key="`${item.file.name}-${item.file.size}-${index}`"
              class="attachment-item"
            >
              <img v-if="item.previewUrl" :src="item.previewUrl" :alt="item.file.name" />
              <span>{{ item.file.name }}</span>
              <el-button text circle aria-label="移除附件" @click="removeAttachment('session', index)">
                <el-icon><Close /></el-icon>
              </el-button>
            </div>
          </div>
          <div class="form-actions">
            <el-button @click="mode = 'chat'">取消</el-button>
            <el-button @click="sessionFileInput?.click()">
              <el-icon><Upload /></el-icon>
              添加文件
            </el-button>
            <el-button type="primary" @click="mode = 'preview'">预览说明</el-button>
          </div>
        </section>

        <section v-else-if="mode === 'preview'" class="preview-page">
          <div class="page-heading">
            <div>
              <p class="eyebrow">新会话</p>
              <h1>确认任务说明</h1>
            </div>
          </div>
          <pre class="markdown-preview">{{ previewMarkdown }}</pre>
          <p v-if="sessionAttachments.length" class="preview-attachments">
            附件：{{ sessionAttachments.map(({ file }) => file.name).join('、') }}
          </p>
          <div class="form-actions">
            <el-button @click="mode = 'editing'">继续编辑</el-button>
            <el-button type="primary" :loading="isSending" @click="confirmNewSession">
              确认并发送
            </el-button>
          </div>
        </section>

        <template v-else-if="activeSession">
          <header class="conversation-heading">
            <div>
              <p class="eyebrow">会话</p>
              <h1>{{ activeSession.session.title || '新会话' }}</h1>
            </div>
            <el-tag size="small" type="info">Codex App Server</el-tag>
          </header>
          <section class="message-list" aria-live="polite">
            <template v-for="message in activeSession.messages" :key="message.id">
              <article
                v-if="message.role === 'user' || message.content"
                class="message-row"
                :data-role="message.role"
              >
                <div class="message-meta">
                  <span>{{ message.role === 'user' ? profile?.name ?? '我' : 'Codex' }}</span>
                  <time>{{ formatTime(message.created_at) }}</time>
                </div>
                <div class="message-content" v-html="renderMarkdown(message.content)"></div>
              </article>
            </template>
            <section v-if="activeSession.activities.length" class="activity-list">
              <details
                v-for="activity in activeSession.activities"
                :key="activity.id"
                class="activity-item"
                :data-status="activity.status"
                :open="activity.status === 'running'"
              >
                <summary>
                  <span class="activity-dot"></span>
                  <span class="activity-title">{{ activity.title }}</span>
                  <time>{{ activityDuration(activity) }}</time>
                </summary>
                <pre v-if="activity.detail" class="activity-detail">{{ activity.detail }}</pre>
                <div v-if="activity.kind === 'approval' && activity.status === 'running'"
                  class="approval-actions">
                  <el-button
                    size="small"
                    type="primary"
                    :loading="respondingApproval === activity.source_id"
                    :disabled="!!respondingApproval"
                    @click.stop="decideApproval(activity, 'accept')"
                  >
                    批准
                  </el-button>
                  <el-button
                    size="small"
                    :disabled="!!respondingApproval"
                    @click.stop="decideApproval(activity, 'decline')"
                  >
                    拒绝
                  </el-button>
                </div>
              </details>
            </section>
            <div v-if="activeSession.session.last_error" class="turn-error" role="alert">
              {{ activeSession.session.last_error }}
            </div>
            <div v-if="isSending" class="working-row" aria-live="polite">
              <span class="working-indicator"></span>
              Codex 正在处理 · {{ turnDuration() }}
              <el-button
                text
                type="danger"
                :loading="isInterrupting"
                :disabled="isInterrupting"
                @click="interruptTurn"
              >
                {{ isInterrupting ? '正在中断' : '强制中断' }}
              </el-button>
            </div>
            <el-empty v-if="!activeSession.messages.length" description="发送第一条消息开始对话" />
          </section>
          <form
            class="composer"
            :class="{ 'file-drop-active': draggingFiles }"
            @submit.prevent="sendMessage"
            @dragover.prevent="draggingFiles = true"
            @dragleave="draggingFiles = false"
            @drop.prevent="dropAttachments($event, 'composer')"
          >
            <input
              ref="composerFileInput"
              class="file-input"
              type="file"
              multiple
              @change="selectAttachments($event, 'composer')"
            />
            <div v-if="composerAttachments.length" class="attachment-list composer-attachments">
              <div
                v-for="(item, index) in composerAttachments"
                :key="`${item.file.name}-${item.file.size}-${index}`"
                class="attachment-item"
              >
                <img v-if="item.previewUrl" :src="item.previewUrl" :alt="item.file.name" />
                <span>{{ item.file.name }}</span>
                <el-button text circle aria-label="移除附件" @click="removeAttachment('composer', index)">
                  <el-icon><Close /></el-icon>
                </el-button>
              </div>
            </div>
            <el-input
              v-model="composerText"
              type="textarea"
              :autosize="{ minRows: 2, maxRows: 8 }"
              placeholder="向 Codex 发送消息"
              :disabled="isSending"
              @paste="pasteAttachments($event, 'composer')"
            />
            <div class="composer-actions">
              <el-button
                aria-label="添加附件"
                title="添加附件"
                :disabled="isSending"
                @click="composerFileInput?.click()"
              >
                <el-icon><Upload /></el-icon>
              </el-button>
              <el-button
                type="primary"
                :disabled="(!composerText.trim() && !composerAttachments.length) || isSending"
                native-type="submit"
              >
                <el-icon><Promotion /></el-icon>
                发送
              </el-button>
            </div>
          </form>
        </template>

        <div v-else-if="loadingSession" class="empty-workspace">
          <el-icon class="loading-icon"><Loading /></el-icon>
        </div>
        <el-empty v-else description="选择一个会话，或新建会话开始工作" />
      </main>
    </div>

    <el-dialog v-model="renameOpen" title="修改显示名称" width="360px">
      <el-input v-model="renameValue" maxlength="40" show-word-limit @keyup.enter="saveName" />
      <template #footer>
        <el-button @click="renameOpen = false">取消</el-button>
        <el-button type="primary" @click="saveName">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>
