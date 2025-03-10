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

package cloud

import (
	"context"
	"time"

	"github.com/deepflowys/deepflow/server/controller/cloud/kubernetes_gather"
	kubernetes_gather_model "github.com/deepflowys/deepflow/server/controller/cloud/kubernetes_gather/model"
	"github.com/deepflowys/deepflow/server/controller/common"
	"github.com/deepflowys/deepflow/server/controller/db/mysql"
)

type KubernetesGatherTask struct {
	kCtx             context.Context
	kCancel          context.CancelFunc
	kubernetesGather *kubernetes_gather.KubernetesGather
	resource         kubernetes_gather_model.KubernetesGatherResource
	basicInfo        kubernetes_gather_model.KubernetesGatherBasicInfo
	SubDomainConfig  string // 附属容器集群配置字段config
}

func NewKubernetesGatherTask(
	domain *mysql.Domain, subDomain *mysql.SubDomain, ctx context.Context, isSubDomain bool) *KubernetesGatherTask {
	kubernetesGather := kubernetes_gather.NewKubernetesGather(domain, subDomain, isSubDomain)
	if kubernetesGather == nil {
		log.Errorf("kubernetes_gather (%s) task init faild", subDomain.Name)
		return nil
	}
	subDomainConfig := ""
	if subDomain != nil {
		subDomainConfig = subDomain.Config
	}

	kCtx, kCancel := context.WithCancel(ctx)
	return &KubernetesGatherTask{
		basicInfo: kubernetes_gather_model.KubernetesGatherBasicInfo{
			Name:                  kubernetesGather.Name,
			Lcuuid:                kubernetesGather.Lcuuid,
			ClusterID:             kubernetesGather.ClusterID,
			PortNameRegex:         kubernetesGather.PortNameRegex,
			PodNetIPv4CIDRMaxMask: kubernetesGather.PodNetIPv4CIDRMaxMask,
			PodNetIPv6CIDRMaxMask: kubernetesGather.PodNetIPv6CIDRMaxMask,
		},
		resource: kubernetes_gather_model.KubernetesGatherResource{
			ErrorState: common.RESOURCE_STATE_CODE_SUCCESS,
		},
		kCtx:             kCtx,
		kCancel:          kCancel,
		kubernetesGather: kubernetesGather,
		SubDomainConfig:  subDomainConfig,
	}
}

func (k *KubernetesGatherTask) GetBasicInfo() kubernetes_gather_model.KubernetesGatherBasicInfo {
	return k.basicInfo
}

func (k *KubernetesGatherTask) GetResource() kubernetes_gather_model.KubernetesGatherResource {
	return k.resource
}

func (k *KubernetesGatherTask) Start() {
	go func() {
		// TODO 配置时间间隔
		ticker := time.NewTicker(time.Minute)
	LOOP:
		for {
			select {
			case <-ticker.C:
				log.Infof("kubernetes gather (%s) assemble data starting", k.kubernetesGather.Name)
				var err error
				k.resource, err = k.kubernetesGather.GetKubernetesGatherData()
				// 这里因为任务内部没有对成功的状态赋值状态码，在这里统一处理了
				if err != nil {
					k.resource.ErrorMessage = err.Error()
					if k.resource.ErrorState == 0 {
						k.resource.ErrorState = common.RESOURCE_STATE_CODE_EXCEPTION
					}
				} else {
					k.resource.ErrorState = common.RESOURCE_STATE_CODE_SUCCESS
				}
				log.Infof("kubernetes gather (%s) assemble data complete", k.kubernetesGather.Name)
			case <-k.kCtx.Done():
				break LOOP
			}
		}
	}()
}

func (k *KubernetesGatherTask) Stop() {
	if k.kCancel != nil {
		k.kCancel()
	}
}
