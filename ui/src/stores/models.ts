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
/** Source kinds accepted by extension preparation. */
export type ExtensionSourceType = "local" | "git" | "oci" | "http"

/** Execution runtimes accepted by the adapter contract. */
export type ExtensionRuntime = "executable" | "python" | "pythonModule"

/** Desired extension configuration; registration does not imply installation. */
export interface ExtensionSpec {
  /** Human-readable name, between one and 128 characters. */
  name: string
  /** Determines how the worker interprets source. */
  sourceType: ExtensionSourceType
  /** Worker-accessible source path or reference. */
  source: string
  /** Maximum total publication attempts, including the initial attempt. */
  maxAttempts?: number
  /** Opaque preparation request token. */
  refreshToken?: string | null
  /** Execution contract packaged with the extension. */
  adapter: {
    /** Pipeline role of this adapter. */
    type: NodeType
    /** Executable or module configuration. */
    execution: {
      /** Executable path or module name. MUST be nonempty. */
      command: string
      /** Defaults to executable on the server. */
      runtime?: ExtensionRuntime
      /** Optional process working directory. */
      workingDirectory?: string | null
    }
    /** Ordered command argument bindings. */
    arguments?: Array<{
      /** Runtime websocket URL or exact settings key. */
      source: string
      /** Omit for positional arguments; switches MUST have a flag. */
      flag?: string | null
      /** Defaults to scalar value binding. */
      mode?: "value" | "switch"
    }>
    /** Setting declarations; omitted defaults differ from explicit null. */
    settings?: Record<string, {
      /** Require a value when no default is declared. */
      required?: boolean
      /** Arbitrary JSON default, including explicit null. */
      default?: unknown
    }>
  }
}

/** Registered resource; status may be absent or describe an older generation. */
export interface Extension {
  /** Resource API version. */
  apiVersion: string
  /** Resource kind. */
  kind: "SuntitleExtension"
  /** Resource identity; metadata.name addresses API requests. */
  metadata: {
    /** Server resource name. */
    name: string
    /** Current desired revision. */
    generation?: number
  }
  /** Desired preparation configuration. */
  spec: ExtensionSpec
  /** Asynchronous publication result; never proves host installation. */
  status?: {
    /** Desired revision represented by this status. */
    observedGeneration?: number | null
    /** Published digest reference. */
    artifactReference?: string | null
    /** Completed publication attempt count. */
    attempts?: number | null
    /** Worker task identifier. */
    taskId?: string | null
    /** Preparation, publication, or terminal failure conditions. */
    conditions?: Array<{
      /** Condition category. */
      type: string
      /** Kubernetes condition truth value. */
      status: string
      /** Machine-readable explanation. */
      reason: string
      /** Human-readable explanation. */
      message: string
      /** Last state transition timestamp. */
      lastTransitionTime: string
      /** Desired revision represented by the condition. */
      observedGeneration?: number
    }>
  } | null
}

/** Extension collection with its server revision. */
export interface ExtensionList {
  /** Registered resources. */
  items: Extension[]
  /** Kubernetes collection revision. */
  resourceVersion: string
}

/** Source-only registration; configuration is read from the extension manifest. */
export interface ExtensionRegistration {
  /** Transport for manifest discovery and publication. */
  sourceType: ExtensionSourceType
  /** Absolute server directory, repository URL, registry reference, or HTTP payload URL. */
  source: string
}

/** Registration request. The server generates a resource name when omitted. */
export interface ExtensionCreate {
  /** Optional Kubernetes resource name. */
  name?: string | null
  /** Desired source and execution contract. */
  spec: ExtensionSpec
}

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
  /** Display name of the referenced extension, independent of its pipeline role. */
  extensionName?: string
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
  /** Resource identity of a custom extension; omission selects a built-in node. */
  extensionRef?: string
  inputNodeId: string | null
  type: NodeType
  name: string
  description?: string | null
  enabled: boolean
  /** Built-in settings or user-defined JSON instance values. */
  settings: Record<string, unknown>
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
  | CustomNodeUpdate

/** Edit custom instance values while retaining extension identity. */
export interface CustomNodeUpdate extends NodeUpdateBase {
  /** Pipeline role resolved from the extension. */
  type: NodeType
  /** Resource name of the referenced extension. */
  extensionRef: string
  /** Arbitrary JSON values declared by the extension manifest. */
  settings: Record<string, unknown>
}

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
  /** Referenced extension for custom settings; absent for built-in nodes. */
  extensionRef?: { name: string } | null
  name: string
  description?: string | null
  hostRef: { name: string }
  enabled: boolean
  inputRef?: { name: string } | null
  restartToken?: string | null
  /** Exactly one built-in variant or the custom JSON setting map. */
  settings: Partial<Record<NodeType, Node["settings"]>> & { custom?: Record<string, unknown> }
}

export interface NodeResource {
  metadata: ResourceMetadata
  spec: NodeSpec
  /** Observed runtime state and the resolved custom adapter role. */
  status?: {
    /** Current lifecycle phase reported by the host controller. */
    phase?: "Pending" | "Running" | "Disabled" | "Failed" | "Disconnected" | "Terminating"
    /** Observed custom execution contract; absent before preparation. */
    resolvedAdapter?: {
      /** Pipeline role preserved independently of custom setting values. */
      type: NodeType
    }
  }
}
