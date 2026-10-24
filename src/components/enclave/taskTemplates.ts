/**
 * 任务模板：把「经常要做的一类事」沉淀成可一键生成的父子任务结构。
 *
 * 口径与 AI 拆解一致：子任务必须是**可独立指派、可独立验收的交付物**，
 * 不是「确认要求 / 审核 / 提交 / 归档」这类流程阶段或状态动作。
 */

export interface TaskTemplateChild {
  title: string;
  /** 交付物内容与验收点 */
  description: string;
  priority?: 'low' | 'normal' | 'high' | 'urgent';
}

export interface TaskTemplate {
  id: string;
  name: string;
  /** 适用场景，帮助用户选对模板 */
  description: string;
  children: TaskTemplateChild[];
}

export const TASK_TEMPLATES: TaskTemplate[] = [
  {
    id: 'report',
    name: '报送材料',
    description: '向上级 / 外部单位报送数据或材料',
    children: [
      {
        title: '整理报送数据表',
        description: '整理本次报送需要的指标与数据，形成可核对的表格（含口径说明）。',
        priority: 'high',
      },
      {
        title: '撰写报送正文',
        description: '按报送要求撰写正文，覆盖背景、数据、结论，形成可提交的初稿。',
        priority: 'high',
      },
      {
        title: '编制附件清单',
        description: '汇总支撑材料与附件，逐项核对文件名与份数，形成附件清单。',
        priority: 'normal',
      },
      {
        title: '形成报送台账',
        description: '记录本次报送对象、时间、版本与联系人，形成可追溯的报送台账。',
        priority: 'low',
      },
    ],
  },
  {
    id: 'release',
    name: '需求上线',
    description: '一个需求从说明到上线验证的交付链',
    children: [
      {
        title: '输出需求说明',
        description: '写清目标、范围与验收标准，形成可评审的需求说明。',
        priority: 'high',
      },
      {
        title: '完成方案设计',
        description: '给出实现方案与影响面（数据 / 接口 / 兼容性），形成设计说明。',
        priority: 'high',
      },
      {
        title: '完成开发与自测',
        description: '按方案实现并自测，附带自测记录与变更清单。',
        priority: 'high',
      },
      {
        title: '输出上线验证报告',
        description: '上线后按验收标准逐项验证，形成验证结论与遗留问题清单。',
        priority: 'normal',
      },
    ],
  },
  {
    id: 'event',
    name: '活动执行',
    description: '线下 / 线上活动的方案、物料与复盘',
    children: [
      {
        title: '输出活动方案',
        description: '明确目标、流程、分工与预算，形成可执行的活动方案。',
        priority: 'high',
      },
      {
        title: '准备物料清单',
        description: '列出所需物料、数量与责任人，形成采购 / 制作清单。',
        priority: 'normal',
      },
      {
        title: '完成现场执行',
        description: '按方案执行并记录现场情况，形成签到表、照片等执行记录。',
        priority: 'high',
      },
      {
        title: '形成复盘报告',
        description: '汇总效果数据与问题，形成可复用的复盘报告。',
        priority: 'low',
      },
    ],
  },
  {
    id: 'research',
    name: '调研报告',
    description: '一次专题调研从提纲到结论',
    children: [
      {
        title: '设计调研提纲',
        description: '确定调研对象、问题清单与取样方式，形成调研提纲。',
        priority: 'normal',
      },
      {
        title: '收集样本数据',
        description: '按提纲收集访谈 / 问卷 / 资料，形成可追溯的原始样本。',
        priority: 'high',
      },
      {
        title: '撰写调研报告',
        description: '按提纲整理分析，形成结构完整的调研报告。',
        priority: 'high',
      },
      {
        title: '汇总结论建议',
        description: '提炼关键结论与可执行建议，形成一页纸摘要。',
        priority: 'normal',
      },
    ],
  },
];
