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

package trisolaris

import (
	"github.com/op/go-logging"
	"gorm.io/gorm"

	"github.com/deepflowys/deepflow/server/controller/trisolaris/config"
	"github.com/deepflowys/deepflow/server/controller/trisolaris/kubernetes"
	"github.com/deepflowys/deepflow/server/controller/trisolaris/metadata"
	"github.com/deepflowys/deepflow/server/controller/trisolaris/node"
	"github.com/deepflowys/deepflow/server/controller/trisolaris/vtap"
)

var log = logging.MustGetLogger("trisolaris")

type Trisolaris struct {
	config         *config.Config
	dbConn         *gorm.DB
	metaData       *metadata.MetaData
	vTapInfo       *vtap.VTapInfo
	nodeInfo       *node.NodeInfo
	kubernetesInfo *kubernetes.KubernetesInfo
}

var trisolaris *Trisolaris

func GetGVTapInfo() *vtap.VTapInfo {
	return trisolaris.vTapInfo
}

func GetGNodeInfo() *node.NodeInfo {
	return trisolaris.nodeInfo
}

func GetGKubernetesInfo() *kubernetes.KubernetesInfo {
	return trisolaris.kubernetesInfo
}

func GetConfig() *config.Config {
	return trisolaris.config
}

func GetDB() *gorm.DB {
	return trisolaris.dbConn
}

func GetBillingMethod() string {
	return trisolaris.config.BillingMethod
}

func PutPlatformData() {
	trisolaris.metaData.PutChPlatformData()
}

func PutTapType() {
	log.Info("PutTapType")
	trisolaris.metaData.PutChTapType()
}

func PutNodeInfo() {
	trisolaris.nodeInfo.PutChNodeInfo()
}

func PutVTapCache() {
	trisolaris.vTapInfo.PutVTapCacheRefresh()
}

func (t *Trisolaris) Start() {
	t.metaData.InitData() // 需要先初始化
	go t.metaData.TimedRefreshMetaData()
	go t.vTapInfo.TimedRefreshVTapCache()
	go t.nodeInfo.TimedRefreshNodeCache()
}

func NewTrisolaris(cfg *config.Config, db *gorm.DB) *Trisolaris {
	if trisolaris == nil {
		cfg.Convert()
		metaData := metadata.NewMetaData(db, cfg)
		trisolaris = &Trisolaris{
			config:         cfg,
			dbConn:         db,
			metaData:       metaData,
			vTapInfo:       vtap.NewVTapInfo(db, metaData, cfg),
			nodeInfo:       node.NewNodeInfo(db, metaData, cfg),
			kubernetesInfo: kubernetes.NewKubernetesInfo(db, cfg),
		}
	} else {
		return trisolaris
	}

	return trisolaris
}
