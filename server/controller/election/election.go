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

// Reference code: https://github.com/kubernetes/client-go/blob/master/examples/leader-election/main.go

package election

import (
	"context"
	"fmt"
	"os"
	"sync"
	"time"

	logging "github.com/op/go-logging"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	clientset "k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"
	"k8s.io/client-go/tools/clientcmd"
	"k8s.io/client-go/tools/leaderelection"
	"k8s.io/client-go/tools/leaderelection/resourcelock"

	"github.com/deepflowys/deepflow/server/controller/common"
	"github.com/deepflowys/deepflow/server/controller/config"
)

const (
	ID_ITEM_NUM = 4
)

type LeaderData struct {
	sync.RWMutex
	Name string
}

func (l *LeaderData) SetLeader(name string) {
	l.Lock()
	l.Name = name
	l.Unlock()
}

func (l *LeaderData) GetLeader() string {
	l.RLock()
	name := l.Name
	l.RUnlock()
	return name
}

var log = logging.MustGetLogger("election")
var leaderData = &LeaderData{}

func buildConfig(kubeconfig string) (*rest.Config, error) {
	if kubeconfig != "" {
		cfg, err := clientcmd.BuildConfigFromFlags("", kubeconfig)
		if err != nil {
			return nil, err
		}
		return cfg, nil
	}

	cfg, err := rest.InClusterConfig()
	if err != nil {
		return nil, err
	}
	return cfg, nil
}

func getID() string {
	return fmt.Sprintf("%s/%s/%s/%s",
		os.Getenv(common.NODE_NAME_KEY),
		os.Getenv(common.NODE_IP_KEY),
		os.Getenv(common.POD_NAME_KEY),
		os.Getenv(common.POD_IP_KEY))
}

func GetLeader() string {
	return leaderData.GetLeader()
}

func Start(ctx context.Context, cfg *config.ControllerConfig) {
	kubeconfig := cfg.Kubeconfig
	electionName := cfg.ElectionName
	electionNamespace := cfg.ElectionNamespace
	id := getID()
	log.Infof("election id is %s", id)
	// leader election uses the Kubernetes API by writing to a
	// lock object, which can be a LeaseLock object (preferred),
	// a ConfigMap, or an Endpoints (deprecated) object.
	// Conflicting writes are detected and each client handles those actions
	// independently.
	config, err := buildConfig(kubeconfig)
	if err != nil {
		log.Fatal(err)
	}

	client := clientset.NewForConfigOrDie(config)

	lock := &resourcelock.LeaseLock{
		LeaseMeta: metav1.ObjectMeta{
			Name:      electionName,
			Namespace: electionNamespace,
		},
		Client: client.CoordinationV1(),
		LockConfig: resourcelock.ResourceLockConfig{
			Identity: id,
		},
	}

	// start the leader election code loop
	leaderelection.RunOrDie(ctx, leaderelection.LeaderElectionConfig{
		Lock: lock,
		// IMPORTANT: you MUST ensure that any code you have that
		// is protected by the lease must terminate **before**
		// you call cancel. Otherwise, you could have a background
		// loop still running and another process could
		// get elected before your background loop finished, violating
		// the stated goal of the lease.
		ReleaseOnCancel: true,
		LeaseDuration:   60 * time.Second,
		RenewDeadline:   15 * time.Second,
		RetryPeriod:     5 * time.Second,
		Callbacks: leaderelection.LeaderCallbacks{
			OnStartedLeading: func(ctx context.Context) {
				// we're notified when we start - this is where you would
				// usually put your code
				log.Infof("%s is the leader", id)
				leaderData.SetLeader(id)
			},
			OnStoppedLeading: func() {
				// we can do cleanup here
				log.Infof("leader lost: %s", id)
				os.Exit(0)
			},
			OnNewLeader: func(identity string) {
				leaderData.SetLeader(identity)
				// we're notified when new leader elected
				log.Infof("new leader elected: %s", identity)
			},
		},
	})
}
