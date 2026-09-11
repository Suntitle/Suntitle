export type NodeType = "audio_source" | "transcriber" | "translator" | "display"
export type NodeSettingInput = "text" | "number" | "select" | "checkbox"
export type NodeSettingValueType = "string" | "integer" | "number" | "boolean"
export type NodeSettingValue = string | number | boolean | null
export type NodeStatus = "connected" | "disconnected" | "degraded"
export type HostStatus = "online" | "offline" | "degraded"
export type ConnectionStatus = "connected" | "reconnecting" | "degraded"
export type DataType = "audio" | "transcription" | "translation"
export type ConnectionDirection = "incoming" | "outgoing"
export type ConnectionProtocol = "websocket" | "http" | "https"
export type JsonMetadata = Record<string, unknown>

export interface NodeSettingOption {
  value: NodeSettingValue
  label: string
}

export interface NodeSettingDefinition {
  key: string
  label: string
  description?: string
  input: NodeSettingInput
  valueType: NodeSettingValueType
  required: boolean
  defaultValue?: NodeSettingValue
  placeholder?: string
  minimum?: number
  maximum?: number
  options?: NodeSettingOption[]
}

export interface NodeTypeDefinition {
  type: NodeType
  name: string
  description: string
  inputTypes: NodeType[]
  settings: NodeSettingDefinition[]
}

export interface NodeTypeDefinitionList {
  items: NodeTypeDefinition[]
  total: number
}

export interface LoginRequest {
  username: string
  password: string
}

export interface User {
  id: string
  username: string
}

export interface LoginResponse {
  accessToken: string
  tokenType: "Bearer"
  expiresIn: number
  user: User
}

export interface Host {
  id: string
  name: string
  ip: string
  version: string
  status: HostStatus
  lastSeenAt: string
  metadata?: JsonMetadata
}

export interface HostList {
  items: HostResource[]
}

export interface NodeConnection {
  id: string
  peerNodeId: string
  peerNodeType: NodeType
  peerName?: string
  direction: ConnectionDirection
  dataType: DataType
  status: ConnectionStatus
  protocol: ConnectionProtocol
  connectedAt: string
  lastActivityAt: string
  bytesTransferred?: number
  metadata?: JsonMetadata
}

export interface AudioSourceSettings {
  sampleRateHz: 8000 | 16000 | 22050 | 44100 | 48000
  channels: number
  encoding: "pcm_s16le" | "pcm_f32le" | "opus"
  deviceName?: string | null
}

export interface TranscriberSettings {
  model: string
  language: string
  task?: "transcribe" | "translate"
  beamSize?: number
  vadEnabled?: boolean
}

export interface TranslatorSettings {
  sourceLanguage: string
  targetLanguage: string
  provider: string
  model?: string | null
  preserveFormatting?: boolean
}

export interface DisplaySettings {
  maxLines: number
  fontScale: number
  showOriginal: boolean
  theme?: "light" | "dark" | "transparent"
}

interface NodeBase {
  id: string
  name: string
  description?: string | null
  enabled: boolean
  status: NodeStatus
  host: Host
  inputNodeId: string | null
  spec: NodeSpec
  createdAt: string
  updatedAt: string
}

export interface AudioSourceNode extends NodeBase {
  type: "audio_source"
  settings: AudioSourceSettings
}

export interface TranscriberNode extends NodeBase {
  type: "transcriber"
  settings: TranscriberSettings
}

export interface TranslatorNode extends NodeBase {
  type: "translator"
  settings: TranslatorSettings
}

export interface DisplayNode extends NodeBase {
  type: "display"
  settings: DisplaySettings
}

export type Node = AudioSourceNode | TranscriberNode | TranslatorNode | DisplayNode

export interface NodeCreate {
  inputNodeId: string | null
  type: NodeType
  name: string
  description?: string | null
  enabled: boolean
  settings: Record<string, NodeSettingValue>
}

interface NodeUpdateBase {
  name: string
  description?: string | null
  enabled: boolean
  inputNodeId: string | null
}

export type NodeUpdate =
  | (NodeUpdateBase & { type: "audio_source"; settings: AudioSourceSettings })
  | (NodeUpdateBase & { type: "transcriber"; settings: TranscriberSettings })
  | (NodeUpdateBase & { type: "translator"; settings: TranslatorSettings })
  | (NodeUpdateBase & { type: "display"; settings: DisplaySettings })

export interface NodeList {
  items: NodeResource[]
}

export interface Problem {
  type: string
  title: string
  status: number
  detail?: string
  instance?: string
  requestId?: string
}

export interface ValidationError {
  path: string
  message: string
}

export interface ValidationProblem extends Problem {
  errors: ValidationError[]
}

export interface ResourceMetadata {
  name: string
  uid: string
  creationTimestamp?: string
}

export interface HostResource {
  metadata: ResourceMetadata
  spec: { name: string }
  status?: {
    ip?: string
    agentVersion?: string
    lastHeartbeat?: string
    reportedMetadata?: JsonMetadata
    presence?: "Online" | "Offline"
  }
}

export interface NodeSpec {
  name: string
  description?: string | null
  hostRef: { name: string }
  enabled: boolean
  inputRef?: { name: string } | null
  restartToken?: string | null
  settings: Partial<Record<NodeType, Node["settings"]>>
}

export interface NodeResource {
  metadata: ResourceMetadata
  spec: NodeSpec
  status?: { phase?: "Pending" | "Running" | "Disabled" | "Failed" | "Disconnected" | "Terminating" }
}
