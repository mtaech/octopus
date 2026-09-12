<script setup lang="ts">
// FieldGrid —— 表单字段网格：按「右侧编辑区」自身宽度（容器查询）自适应列数，
// 因此左侧清单开合 / 校验 dock 展开都不会把字段挤扁。
// dense：行距收紧一档（信息密集的实体表单，如「人物」）。
withDefaults(defineProps<{ dense?: boolean }>(), { dense: false })
</script>

<template>
  <div class="grid grid-cols-1 gap-x-4 @lg:grid-cols-2 @3xl:grid-cols-3" :class="dense ? 'gap-y-2' : 'gap-y-3.5'">
    <slot />
  </div>
</template>
