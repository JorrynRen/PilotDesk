/**
 * 飞地（Enclave）前端类型：与 Rust serde 输出对齐（`rename_all = "camelCase"`）。
 *
 * 权威分工（见设计文档《飞地（Enclave）产品设计与落地方案》）：
 * 组织/成员/角色以平台为准（这里只读快照）；任务/汇报/产出/文件以主机为准。
 */

/** 飞地模块状态（`enclave_status`）。 */
export interface EnclaveStatus {
  /** 飞地数据库是否已打开并就绪 */
  ready: boolean;
  /** `enclave.db` 的绝对路径 */
  dbPath: string;
  /** 打开时确定的结构版本 */
  schemaVersion: number;
}
