/*
 * Copyright (c) 2022 Yunshan Networks
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package tagrecorder

import (
	"time"

	logging "github.com/op/go-logging"

	// "github.com/deepflowys/deepflow/server/controller/tagrecorder/config"
	"github.com/deepflowys/deepflow/server/controller/config"
)

var log = logging.MustGetLogger("tagrecorder")

type TagRecorder struct {
	cfg config.ControllerConfig
}

func NewTagRecorder(cfg config.ControllerConfig) *TagRecorder {
	return &TagRecorder{cfg: cfg}
}

// 每次执行需要做的事情
func (c *TagRecorder) run() {
	log.Info("tagrecorder run")

	// 连接数据节点刷新ClickHouse中的字典定义
	c.UpdateChDictionary()
	// 调用API获取资源对应的icon_id
	domainToIconID, resourceToIconID, _ := c.UpdateIconInfo()
	c.refresh(domainToIconID, resourceToIconID)
}

func (c *TagRecorder) Start() {
	go func() {
		for range time.Tick(time.Duration(c.cfg.TagRecorderCfg.Interval) * time.Second) {
			c.run()
		}
	}()
}

func (c *TagRecorder) refresh(domainLcuuidToIconID map[string]int, resourceTypeToIconID map[IconKey]int) {
	// 生成各资源更新器，刷新ch数据
	updaters := []ChResourceUpdater{
		NewChRegion(domainLcuuidToIconID, resourceTypeToIconID),
		NewChAZ(domainLcuuidToIconID, resourceTypeToIconID),
		NewChVPC(resourceTypeToIconID),
		NewChDevice(resourceTypeToIconID),
		NewChIPRelation(),
		NewChDevicePort(),
		NewChPodPort(),
		NewChPodNodePort(),
		NewChPodGroupPort(),
		NewChIPPort(),
		NewChK8sLabel(),
		NewChK8sLabels(),
		NewChVTapPort(),
		NewChNetwork(resourceTypeToIconID),
		NewChTapType(resourceTypeToIconID),
		NewChVTap(resourceTypeToIconID),
		NewChPod(resourceTypeToIconID),
		NewChPodCluster(resourceTypeToIconID),
		NewChPodGroup(resourceTypeToIconID),
		NewChPodNamespace(resourceTypeToIconID),
		NewChPodNode(resourceTypeToIconID),
		NewChLbListener(resourceTypeToIconID),
		NewChPodIngress(resourceTypeToIconID),
	}
	if c.cfg.RedisCfg.Enabled {
		updaters = append(updaters, NewChIPResource())
	}
	for _, updater := range updaters {
		updater.Refresh()
	}
}
